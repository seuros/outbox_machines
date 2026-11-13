//! Randomized runs against an independent naive model.

use super::*;

/// Independent reference: a plain vector with the same documented policy.
#[derive(Clone)]
struct Model {
    rows: Vec<(u64, State, u32, u64, u64, u64)>, // id, state, attempts, avail, lease, seq
    cap: usize,
    max: u32,
    seq: u64,
}

impl Model {
    fn settle(&mut self, i: usize, avail: u64) {
        let max = self.max;
        let row = &mut self.rows[i];
        if row.2 >= max {
            row.1 = State::Dead;
        } else {
            row.1 = State::Pending;
            row.3 = avail;
        }
    }
}

struct Rng(u64);

impl Rng {
    fn next(&mut self, bound: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) % bound
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn behaves_like_the_reference_model() {
    for seed in 0..64 {
        let mut rng = Rng(seed);
        let mut outbox = Outbox::<5>::new(max(3));
        let mut model = Model {
            rows: Vec::new(),
            cap: 5,
            max: 3,
            seq: 0,
        };
        let mut now = 0u64;
        for _ in 0..300 {
            now += rng.next(4);
            let id = rng.next(8);
            match rng.next(6) {
                0 => {
                    let got = outbox.enqueue(id, at(now));
                    let want = if model.rows.iter().any(|r| r.0 == id) {
                        Enqueue::Duplicate
                    } else if model.rows.len() >= model.cap {
                        Enqueue::Full
                    } else {
                        model.rows.push((id, State::Pending, 0, now, 0, model.seq));
                        model.seq += 1;
                        Enqueue::Enqueued
                    };
                    assert_eq!(got, Ok(want));
                }
                1 => {
                    let lease = 1 + rng.next(6);
                    let want = model
                        .rows
                        .iter()
                        .enumerate()
                        .filter(|(_, r)| r.1 == State::Pending && r.3 <= now)
                        .min_by_key(|(_, r)| r.5)
                        .map(|(i, _)| i);
                    let got = outbox.claim(at(now), at(lease));
                    let want = want.map(|i| {
                        let row = &mut model.rows[i];
                        row.1 = State::Claimed;
                        row.2 += 1;
                        row.4 = now + lease;
                        (row.0, row.2)
                    });
                    assert_eq!(got.map(|c| c.map(|c| (c.id, c.generation))), Ok(want));
                }
                2 | 3 => {
                    let generation = u32::try_from(1 + rng.next(4)).unwrap_or(1);
                    let confirm = rng.next(2) == 0;
                    let retry = rng.next(5);
                    let pos = model.rows.iter().position(|r| r.0 == id);
                    let live = pos.filter(|&i| {
                        model.rows[i].1 == State::Claimed && model.rows[i].2 == generation
                    });
                    let got = if confirm {
                        outbox.confirm(id, generation).map(|()| None)
                    } else {
                        outbox.fail(id, generation, at(now), at(retry)).map(Some)
                    };
                    let want = match (pos, live) {
                        (None, _) => Err(Refusal::Unknown),
                        (Some(_), None) => Err(Refusal::Stale),
                        (Some(_), Some(i)) if confirm => {
                            model.rows[i].1 = State::Confirmed;
                            Ok(None)
                        }
                        (Some(_), Some(i)) => {
                            model.settle(i, now + retry);
                            Ok(Some(if model.rows[i].1 == State::Dead {
                                Disposition::Dead
                            } else {
                                Disposition::Retry(at(now + retry))
                            }))
                        }
                    };
                    assert_eq!(got, want, "seed {seed}");
                }
                4 => {
                    let mut out = [Expired {
                        id: 0,
                        generation: 0,
                        disposition: Disposition::Dead,
                    }; 5];
                    // Budget of N slots from any cursor position may be partial;
                    // loop to a full pass and compare the reaped set.
                    let mut reaped = Vec::new();
                    loop {
                        let sweep = outbox.expire_claims(at(now), 2, &mut out);
                        let Ok(sweep) = sweep else {
                            panic!("time is monotonic")
                        };
                        reaped.extend(out[..sweep.written].iter().map(|e| e.id));
                        if sweep.complete {
                            break;
                        }
                    }
                    reaped.sort_unstable();
                    let mut want = Vec::new();
                    for i in 0..model.rows.len() {
                        if model.rows[i].1 == State::Claimed && model.rows[i].4 <= now {
                            want.push(model.rows[i].0);
                            model.settle(i, now);
                        }
                    }
                    want.sort_unstable();
                    assert_eq!(reaped, want, "seed {seed}");
                }
                _ => {
                    let got = outbox.forget(id);
                    let pos = model.rows.iter().position(|r| r.0 == id);
                    let want = match pos {
                        None => Err(Refusal::Unknown),
                        Some(i) if matches!(model.rows[i].1, State::Confirmed | State::Dead) => {
                            model.rows.remove(i);
                            Ok(())
                        }
                        Some(_) => Err(Refusal::NotTerminal),
                    };
                    assert_eq!(got, want);
                }
            }
            assert_eq!(outbox.len(), model.rows.len());
            for row in &model.rows {
                let entry = outbox.get(row.0).map(|e| (e.state, e.attempts));
                assert_eq!(entry, Some((row.1, row.2)), "seed {seed}");
            }
        }
    }
}
