use super::*;
use core::num::NonZeroU32;
use std::vec::Vec;

fn at(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

fn max(n: u32) -> NonZeroU32 {
    NonZeroU32::new(n).unwrap_or(NonZeroU32::MIN)
}

fn claim<const N: usize>(outbox: &mut Outbox<N>, now: u64, lease: u64) -> Claimed {
    match outbox.claim(at(now), at(lease)) {
        Ok(Some(claimed)) => claimed,
        other => panic!("expected a claim, got {other:?}"),
    }
}

#[test]
fn happy_path_confirms_exactly_once() {
    let mut outbox = Outbox::<2>::new(max(3));
    assert_eq!(outbox.enqueue(7, at(0)), Ok(Enqueue::Enqueued));
    let claimed = claim(&mut outbox, 0, 10);
    assert_eq!((claimed.id, claimed.generation), (7, 1));
    assert_eq!(claimed.lease_expires, at(10));
    assert_eq!(outbox.confirm(7, 1), Ok(()));
    assert_eq!(outbox.confirm(7, 1), Err(Error::Stale));
    assert_eq!(outbox.get(7).map(|e| e.state), Some(State::Confirmed));
    assert_eq!(outbox.claim(at(1), at(10)), Ok(None));
}

#[test]
fn enqueue_refuses_duplicates_and_overflow_without_evicting() {
    let mut outbox = Outbox::<1>::new(max(1));
    assert_eq!(outbox.enqueue(1, at(0)), Ok(Enqueue::Enqueued));
    assert_eq!(outbox.enqueue(1, at(0)), Ok(Enqueue::Duplicate));
    assert_eq!(outbox.enqueue(2, at(0)), Ok(Enqueue::Full));
    assert_eq!(outbox.len(), 1);
    assert_eq!(
        Outbox::<0>::new(max(1)).enqueue(1, at(0)),
        Ok(Enqueue::Full)
    );
    // Terminal records keep their id until forgotten.
    let c = claim(&mut outbox, 0, 5);
    assert_eq!(outbox.confirm(c.id, c.generation), Ok(()));
    assert_eq!(outbox.enqueue(1, at(1)), Ok(Enqueue::Duplicate));
    assert_eq!(outbox.forget(1), Ok(()));
    assert_eq!(outbox.enqueue(1, at(1)), Ok(Enqueue::Enqueued));
}

#[test]
fn forget_only_drops_terminal_records() {
    let mut outbox = Outbox::<2>::new(max(1));
    assert_eq!(outbox.forget(1), Err(Error::Unknown));
    assert_eq!(outbox.enqueue(1, at(0)), Ok(Enqueue::Enqueued));
    assert_eq!(outbox.forget(1), Err(Error::NotTerminal));
    let c = claim(&mut outbox, 0, 5);
    assert_eq!(outbox.forget(1), Err(Error::NotTerminal));
    assert_eq!(
        outbox.fail(1, c.generation, at(0), at(0)),
        Ok(Disposition::Dead)
    );
    assert_eq!(outbox.forget(1), Ok(()));
    assert!(outbox.is_empty());
}

#[test]
fn claims_are_fifo_among_due_records() {
    let mut outbox = Outbox::<3>::new(max(3));
    for id in [30, 10, 20] {
        assert_eq!(outbox.enqueue(id, at(0)), Ok(Enqueue::Enqueued));
    }
    let order: Vec<u64> = (0..3).map(|_| claim(&mut outbox, 0, 5).id).collect();
    assert_eq!(order, [30, 10, 20]);
}

#[test]
fn fail_retries_after_delay_then_dies_at_max_attempts() {
    let mut outbox = Outbox::<1>::new(max(2));
    assert_eq!(outbox.enqueue(1, at(0)), Ok(Enqueue::Enqueued));
    let first = claim(&mut outbox, 0, 10);
    assert_eq!(
        outbox.fail(1, first.generation, at(1), at(50)),
        Ok(Disposition::Retry(at(51)))
    );
    assert_eq!(outbox.claim(at(50), at(10)), Ok(None));
    let second = claim(&mut outbox, 51, 10);
    assert_eq!(second.generation, 2);
    assert_eq!(
        outbox.fail(1, second.generation, at(52), at(50)),
        Ok(Disposition::Dead)
    );
    assert_eq!(outbox.count(State::Dead), 1);
    assert_eq!(outbox.claim(at(1_000), at(10)), Ok(None));
}

#[test]
fn stale_generations_cannot_touch_a_newer_attempt() {
    let mut outbox = Outbox::<1>::new(max(5));
    assert_eq!(outbox.enqueue(1, at(0)), Ok(Enqueue::Enqueued));
    let old = claim(&mut outbox, 0, 10);
    let mut out = [Expired {
        id: 0,
        generation: 0,
        disposition: Disposition::Dead,
    }; 1];
    let sweep = outbox.expire_claims(at(10), 8, &mut out);
    assert_eq!(sweep.map(|s| s.written), Ok(1));
    let fresh = claim(&mut outbox, 10, 10);
    assert_eq!(fresh.generation, 2);
    assert_eq!(outbox.confirm(1, old.generation), Err(Error::Stale));
    assert_eq!(
        outbox.fail(1, old.generation, at(10), at(0)),
        Err(Error::Stale)
    );
    assert_eq!(outbox.confirm(2, 1), Err(Error::Unknown));
    assert_eq!(outbox.confirm(1, fresh.generation), Ok(()));
}

#[test]
fn late_confirm_is_accepted_until_the_lease_is_reaped() {
    let blank = Expired {
        id: 0,
        generation: 0,
        disposition: Disposition::Dead,
    };
    let mut outbox = Outbox::<3>::new(max(2));
    assert_eq!(outbox.enqueue(1, at(0)), Ok(Enqueue::Enqueued));
    assert_eq!(outbox.enqueue(2, at(0)), Ok(Enqueue::Enqueued));
    let kept = claim(&mut outbox, 0, 5);
    let lost = claim(&mut outbox, 0, 5);
    // The clock moves far past both deadlines without a sweep.
    assert_eq!(outbox.enqueue(3, at(100)), Ok(Enqueue::Enqueued));
    assert_eq!(outbox.confirm(kept.id, kept.generation), Ok(()));
    let mut out = [blank; 3];
    let sweep = outbox.expire_claims(at(100), 3, &mut out);
    assert_eq!(sweep.map(|s| (s.written, s.complete)), Ok((1, true)));
    assert_eq!(
        out[0],
        Expired {
            id: lost.id,
            generation: lost.generation,
            disposition: Disposition::Retry(at(100)),
        }
    );
    // Once reaped the attempt is superseded, even before a new claim.
    assert_eq!(outbox.confirm(lost.id, lost.generation), Err(Error::Stale));
    assert_eq!(outbox.get(lost.id).map(|e| e.state), Some(State::Pending));
}

#[test]
fn refusals_leave_the_table_untouched() {
    let mut outbox = Outbox::<2>::new(max(2));
    assert_eq!(outbox.enqueue(u64::MAX, at(10)), Ok(Enqueue::Enqueued));
    assert_eq!(outbox.enqueue(0, at(10)), Ok(Enqueue::Enqueued));
    let c = claim(&mut outbox, 10, 5);
    assert_eq!(c.id, u64::MAX);
    let before = std::format!("{outbox:?}");
    let mut out = [];
    assert_eq!(outbox.enqueue(5, at(9)), Err(Error::TimeRegression));
    assert_eq!(outbox.enqueue(5, at(10)), Ok(Enqueue::Full));
    assert_eq!(outbox.enqueue(0, at(10)), Ok(Enqueue::Duplicate));
    assert_eq!(outbox.claim(at(9), at(1)), Err(Error::TimeRegression));
    assert_eq!(
        outbox.claim(at(10), Duration::ZERO),
        Err(Error::InvalidDuration)
    );
    assert_eq!(
        outbox.claim(at(10), Duration::MAX),
        Err(Error::InvalidDuration)
    );
    assert_eq!(outbox.confirm(7, 1), Err(Error::Unknown));
    assert_eq!(outbox.confirm(u64::MAX, 2), Err(Error::Stale));
    assert_eq!(outbox.confirm(u64::MAX, 0), Err(Error::Stale));
    assert_eq!(outbox.confirm(0, 0), Err(Error::Stale));
    assert_eq!(outbox.confirm(0, 1), Err(Error::Stale));
    assert_eq!(outbox.fail(7, 1, at(10), at(0)), Err(Error::Unknown));
    assert_eq!(
        outbox.fail(u64::MAX, u32::MAX, at(10), at(0)),
        Err(Error::Stale)
    );
    assert_eq!(
        outbox.fail(u64::MAX, 1, at(9), at(0)),
        Err(Error::TimeRegression)
    );
    assert_eq!(
        outbox.fail(u64::MAX, 1, at(10), Duration::MAX),
        Err(Error::InvalidDuration)
    );
    assert_eq!(
        outbox.expire_claims(at(9), 2, &mut out),
        Err(Error::TimeRegression)
    );
    assert_eq!(outbox.forget(7), Err(Error::Unknown));
    assert_eq!(outbox.forget(0), Err(Error::NotTerminal));
    assert_eq!(outbox.forget(u64::MAX), Err(Error::NotTerminal));
    assert_eq!(std::format!("{outbox:?}"), before);
}

#[test]
fn expire_claims_is_bounded_and_resumable() {
    let mut outbox = Outbox::<4>::new(max(1));
    for id in 0..4 {
        assert_eq!(outbox.enqueue(id, at(0)), Ok(Enqueue::Enqueued));
        claim(&mut outbox, 0, 10);
    }
    let blank = Expired {
        id: 0,
        generation: 0,
        disposition: Disposition::Dead,
    };
    let mut out = [blank; 1];
    let mut seen = Vec::new();
    let mut calls = 0;
    loop {
        let sweep = outbox.expire_claims(at(10), 8, &mut out).unwrap_or(Sweep {
            written: 0,
            scanned: 0,
            complete: true,
        });
        calls += 1;
        seen.extend(out[..sweep.written].iter().map(|e| e.id));
        assert!(calls < 20, "sweep must terminate");
        if sweep.complete {
            break;
        }
    }
    seen.sort_unstable();
    assert_eq!(seen, [0, 1, 2, 3]);
    assert_eq!(outbox.count(State::Dead), 4);
    // A scan budget of zero makes no progress and never reports completion.
    let idle = outbox.expire_claims(at(10), 0, &mut out);
    assert_eq!(idle.map(|s| (s.scanned, s.complete)), Ok((0, false)));
    // Zero-capacity tables complete trivially.
    let mut empty = Outbox::<0>::new(max(1));
    assert_eq!(
        empty.expire_claims(at(0), 1, &mut out).map(|s| s.complete),
        Ok(true)
    );
}

#[test]
fn time_and_duration_refusals_do_not_mutate() {
    let mut outbox = Outbox::<2>::new(max(3));
    assert_eq!(outbox.enqueue(1, at(10)), Ok(Enqueue::Enqueued));
    assert_eq!(outbox.enqueue(2, at(9)), Err(Error::TimeRegression));
    assert_eq!(outbox.claim(at(9), at(1)), Err(Error::TimeRegression));
    assert_eq!(
        outbox.claim(at(10), Duration::ZERO),
        Err(Error::InvalidDuration)
    );
    assert_eq!(
        outbox.claim(Duration::MAX, at(1)),
        Err(Error::InvalidDuration)
    );
    assert_eq!(outbox.get(1).map(|e| e.state), Some(State::Pending));
    let c = claim(&mut outbox, 10, 5);
    assert_eq!(
        outbox.fail(1, c.generation, at(9), at(0)),
        Err(Error::TimeRegression)
    );
    assert_eq!(
        outbox.fail(1, c.generation, Duration::MAX, at(1)),
        Err(Error::InvalidDuration)
    );
    assert_eq!(outbox.get(1).map(|e| e.state), Some(State::Claimed));
    let mut out = [];
    assert_eq!(
        outbox.expire_claims(at(9), 1, &mut out),
        Err(Error::TimeRegression)
    );
}

#[test]
fn sequence_exhaustion_is_refused() {
    let mut outbox = Outbox::<1>::new(max(1));
    outbox.next_seq = u64::MAX;
    assert_eq!(outbox.enqueue(1, at(0)), Err(Error::Exhausted));
    assert!(outbox.is_empty());
}

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
                        (None, _) => Err(Error::Unknown),
                        (Some(_), None) => Err(Error::Stale),
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
                        None => Err(Error::Unknown),
                        Some(i) if matches!(model.rows[i].1, State::Confirmed | State::Dead) => {
                            model.rows.remove(i);
                            Ok(())
                        }
                        Some(_) => Err(Error::NotTerminal),
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
