//! Randomized runs against an independent naive model.

use super::*;
use periscope_machines::{scope_of, scoped, sequence_of};

#[derive(Clone, Copy)]
struct Row {
    id: u64,
    state: State,
    /// Claims this record has had.
    attempts: u32,
    /// Identity of the latest claim; 0 before the first.
    attempt: u64,
    avail: u64,
    lease: u64,
    /// Enqueue sequence: also names the life of the id.
    seq: u64,
    /// `retry_after` of the `fail` that ended the latest attempt.
    failed: Option<u64>,
}

/// Independent reference: a plain vector with the same documented policy.
#[derive(Clone)]
struct Model {
    rows: Vec<Row>,
    cap: usize,
    max: u32,
    seq: u64,
    scope: u16,
    /// Sequence number of the next attempt identity.
    next_attempt: u64,
    observed: u64,
}

impl Model {
    fn settle(&mut self, i: usize, avail: u64) -> Disposition {
        let max = self.max;
        let row = &mut self.rows[i];
        if row.attempts >= max {
            row.state = State::Dead;
            Disposition::Dead
        } else {
            row.state = State::Pending;
            row.avail = avail;
            Disposition::Retry(at(avail))
        }
    }

    fn enqueue(&mut self, id: u64, now: u64) -> Result<Enqueue, Refusal> {
        if now < self.observed {
            Err(Refusal::TimeRegression)
        } else if self.rows.iter().any(|r| r.id == id) {
            Ok(Enqueue::Duplicate)
        } else if self.rows.len() >= self.cap {
            Err(Refusal::Full)
        } else {
            self.rows.push(Row {
                id,
                state: State::Pending,
                attempts: 0,
                attempt: 0,
                avail: now,
                lease: 0,
                seq: self.seq,
                failed: None,
            });
            self.seq += 1;
            self.observed = now;
            Ok(Enqueue::Enqueued)
        }
    }

    /// The claimed id and attempt identity, with the life it was issued in.
    fn claim(&mut self, now: u64, lease: u64) -> Option<Issued> {
        self.observed = now;
        let attempt = scoped(self.scope, self.next_attempt);
        let row = self
            .rows
            .iter_mut()
            .filter(|r| r.state == State::Pending && r.avail <= now)
            .min_by_key(|r| r.seq)?;
        row.state = State::Claimed;
        row.attempts += 1;
        row.attempt = attempt;
        row.lease = now + lease;
        row.failed = None;
        self.next_attempt += 1;
        Some(Issued {
            id: row.id,
            attempt,
            life: row.seq,
        })
    }

    /// Whether this outbox handed `attempt` out: its scope, a sequence reached.
    fn issued(&self, attempt: u64) -> bool {
        let sequence = sequence_of(attempt);
        scope_of(attempt) == self.scope && sequence != 0 && sequence < self.next_attempt
    }

    fn confirm(&mut self, id: u64, attempt: u64) -> Result<Confirmation, Refusal> {
        if !self.issued(attempt) {
            return Err(Refusal::Stale);
        }
        let row = self.rows.iter_mut().find(|r| r.id == id);
        let row = row.ok_or(Refusal::Unknown)?;
        match row.state {
            _ if row.attempt != attempt => Err(Refusal::Stale),
            State::Claimed => {
                row.state = State::Confirmed;
                Ok(Confirmation::Confirmed)
            }
            State::Confirmed => Ok(Confirmation::AlreadyConfirmed),
            State::Pending | State::Dead => Err(Refusal::Stale),
        }
    }

    fn fail(&mut self, id: u64, attempt: u64, now: u64, retry: u64) -> Result<Failure, Refusal> {
        if !self.issued(attempt) {
            return Err(Refusal::Stale);
        }
        let i = self.rows.iter().position(|r| r.id == id);
        let i = i.ok_or(Refusal::Unknown)?;
        let row = self.rows[i];
        if row.attempt != attempt || row.state == State::Confirmed {
            return Err(Refusal::Stale);
        }
        if row.state != State::Claimed && row.failed.is_none() {
            return Err(Refusal::Stale);
        }
        if now < self.observed {
            return Err(Refusal::TimeRegression);
        }
        match row.failed {
            // Ended by an earlier `fail`: the standing answer, same payload.
            Some(recorded) if recorded != retry => Err(Refusal::Mismatch),
            Some(_) if row.state == State::Dead => Ok(Failure::AlreadyFailed(Disposition::Dead)),
            Some(_) => Ok(Failure::AlreadyFailed(Disposition::Retry(at(row.avail)))),
            None => {
                self.observed = now;
                self.rows[i].failed = Some(retry);
                Ok(Failure::Failed(self.settle(i, now + retry)))
            }
        }
    }

    fn forget(&mut self, id: u64) -> Result<Forgetting, Refusal> {
        match self.rows.iter().position(|r| r.id == id) {
            None => Ok(Forgetting::NotHeld),
            Some(i) if matches!(self.rows[i].state, State::Confirmed | State::Dead) => {
                self.rows.remove(i);
                Ok(Forgetting::Forgotten)
            }
            Some(_) => Err(Refusal::NotTerminal),
        }
    }

    /// `true` when the id is held by a record other than the one `life` names:
    /// it was forgotten and enqueued again since the attempt was issued.
    fn outlived(&self, issued: Issued) -> bool {
        let row = self.rows.iter().find(|r| r.id == issued.id);
        row.is_some_and(|r| r.seq != issued.life)
    }
}

/// An attempt the outbox issued, and the life of the id it was issued in.
#[derive(Clone, Copy)]
struct Issued {
    id: u64,
    attempt: u64,
    life: u64,
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

/// What the run must have exercised, counted across every seed.
#[derive(Debug, Default)]
struct Seen {
    already_confirmed: usize,
    already_failed: usize,
    not_held: usize,
    duplicate: usize,
    mismatch: usize,
    /// Late confirms and fails naming an attempt of an earlier life of an id
    /// that is held again; the second counts those aimed at a live claim.
    old_life: usize,
    old_life_on_claimed: usize,
    /// Reports carrying an attempt of another scope whose sequence number is
    /// the latest issued here; the second counts those aimed at a held id.
    foreign: usize,
    foreign_on_held: usize,
    /// In scope but never issued, naming an id no record carries: the report
    /// a scope comparison alone would answer `Unknown`.
    never_issued_on_absent: usize,
}

#[test]
fn behaves_like_the_reference_model() {
    let mut tally = Seen::default();
    for seed in 0..64 {
        // Scope zero, a middle scope and the last one all behave alike.
        let scope = [0, 0x0102, u16::MAX][usize::try_from(seed % 3).unwrap_or(0)];
        run(seed, scope, &mut tally);
    }
    let counts = [
        tally.already_confirmed,
        tally.already_failed,
        tally.not_held,
        tally.duplicate,
        tally.mismatch,
        tally.old_life,
        tally.old_life_on_claimed,
        tally.foreign,
        tally.foreign_on_held,
        tally.never_issued_on_absent,
    ];
    assert!(counts.iter().all(|&n| n > 20), "tally: {tally:?}");
}

fn run(seed: u64, scope: u16, tally: &mut Seen) {
    let mut rng = Rng(seed);
    let mut outbox = Outbox::<5>::with_scope(max(3), scope);
    assert_eq!(outbox.scope(), scope);
    let mut model = Model {
        rows: Vec::new(),
        cap: 5,
        max: 3,
        seq: 0,
        scope,
        next_attempt: 1,
        observed: 0,
    };
    let mut clock = 0u64;
    // Every attempt ever issued: late reports are drawn from here, so they
    // outlive the forgetting and re-enqueueing of their id.
    let mut issued: Vec<Issued> = Vec::new();
    for _ in 0..400 {
        clock += rng.next(4);
        // One call in four is handed a possibly stale instant.
        let now = if rng.next(4) == 0 {
            clock.saturating_sub(rng.next(6))
        } else {
            clock
        };
        let id = rng.next(8);
        let before = outbox.clone();
        let changed = match rng.next(6) {
            0 => {
                let got = outbox.enqueue(id, at(now));
                assert_eq!(got, model.enqueue(id, now), "seed {seed}");
                tally.duplicate += usize::from(got == Ok(Enqueue::Duplicate));
                got == Ok(Enqueue::Enqueued)
            }
            1 => {
                let lease = 1 + rng.next(6);
                let got = outbox.claim(at(now), at(lease));
                let got = got.map(|c| c.map(|c| (c.id, c.attempt)));
                if now < model.observed {
                    assert_eq!(got, Err(Refusal::TimeRegression), "seed {seed}");
                    false
                } else {
                    // Honoured even when nothing is due: the instant counts.
                    let want = model.claim(now, lease);
                    assert_eq!(got, Ok(want.map(|c| (c.id, c.attempt))), "seed {seed}");
                    issued.extend(want);
                    true
                }
            }
            2 | 3 => report(&mut rng, &mut outbox, &mut model, &issued, (id, now), tally),
            4 if now < model.observed => {
                let got = outbox.expire_claims(at(now), 2, &mut []);
                assert_eq!(got, Err(Refusal::TimeRegression), "seed {seed}");
                false
            }
            4 => {
                sweep(&mut outbox, &mut model, now);
                true
            }
            _ => {
                let got = outbox.forget(id);
                assert_eq!(got, model.forget(id), "seed {seed}");
                tally.not_held += usize::from(got == Ok(Forgetting::NotHeld));
                got == Ok(Forgetting::Forgotten)
            }
        };
        // A refusal or an idempotent repeat leaves the whole value alone,
        // observed clock and sweep cursor included.
        if !changed {
            assert_eq!(outbox, before, "seed {seed}");
        }
        assert_eq!(outbox.observed, at(model.observed), "seed {seed}");
        assert_eq!(outbox.len(), model.rows.len());
        for row in &model.rows {
            let entry = outbox.get(row.id).map(|e| (e.state, e.attempts, e.attempt));
            let attempt = (row.attempt != 0).then_some(row.attempt);
            assert_eq!(
                entry,
                Some((row.state, row.attempts, attempt)),
                "seed {seed}"
            );
        }
    }
}

/// One `confirm` or `fail`; `true` when it changed the outbox.
fn report(
    rng: &mut Rng,
    outbox: &mut Outbox<5>,
    model: &mut Model,
    issued: &[Issued],
    (id, now): (u64, u64),
    tally: &mut Seen,
) -> bool {
    let confirm = rng.next(3) == 0;
    // Few distinct delays, so a repeated `fail` both matches and mismatches.
    let retry = rng.next(3);
    // Half the reports name the latest claim, so that repeats of one attempt
    // are common; a quarter replay any attempt ever issued, however old; the
    // rest are guesses.
    let guess = Issued {
        id,
        attempt: scoped(model.scope, rng.next(model.next_attempt + 2)),
        life: u64::MAX,
    };
    let named = match (rng.next(4), issued.last()) {
        (0 | 1, Some(latest)) => *latest,
        (2, Some(_)) => {
            let pick = rng.next(issued.len() as u64);
            issued[usize::try_from(pick).unwrap_or(0)]
        }
        (0..=2, None) | (3.., _) => guess,
    };
    // One report in eight comes from an outbox of another scope that issued
    // the same sequence number: the same record id, a different identity.
    let named = if rng.next(8) == 0 {
        tally.foreign += 1;
        // Flipping low scope bits always lands in another scope.
        let flip = [1u16, 2, 3][usize::try_from(rng.next(3)).unwrap_or(0)];
        tally.foreign_on_held += usize::from(model.rows.iter().any(|r| r.id == named.id));
        Issued {
            attempt: named.attempt ^ scoped(flip, 0),
            life: u64::MAX,
            ..named
        }
    } else {
        named
    };
    let foreign = scope_of(named.attempt) != model.scope;
    let old_life = named.life != u64::MAX && model.outlived(named);
    let on_claimed = model
        .rows
        .iter()
        .any(|r| r.id == named.id && r.state == State::Claimed);
    if old_life {
        tally.old_life += 1;
        tally.old_life_on_claimed += usize::from(on_claimed);
    }
    let Issued { id, attempt, .. } = named;
    let never_issued = !foreign && !model.issued(attempt);
    let absent = model.rows.iter().all(|r| r.id != id);
    tally.never_issued_on_absent += usize::from(never_issued && absent);
    if confirm {
        let got = outbox.confirm(id, attempt);
        assert_eq!(got, model.confirm(id, attempt));
        // An attempt of an earlier life never settles the record held now.
        assert!(!(old_life || foreign || never_issued) || got == Err(Refusal::Stale));
        tally.already_confirmed += usize::from(got == Ok(Confirmation::AlreadyConfirmed));
        got == Ok(Confirmation::Confirmed)
    } else {
        let got = outbox.fail(id, attempt, at(now), at(retry));
        assert_eq!(got, model.fail(id, attempt, now, retry));
        assert!(!(old_life || foreign || never_issued) || got == Err(Refusal::Stale));
        tally.already_failed += usize::from(matches!(got, Ok(Failure::AlreadyFailed(_))));
        tally.mismatch += usize::from(got == Err(Refusal::Mismatch));
        matches!(got, Ok(Failure::Failed(_)))
    }
}

/// Sweep to a full pass and compare the reaped set with the model's.
fn sweep(outbox: &mut Outbox<5>, model: &mut Model, now: u64) {
    let mut out = [Expired::default(); 5];
    model.observed = now;
    // A budget of two slots is partial from any cursor position.
    let mut reaped = Vec::new();
    loop {
        let sweep = outbox.expire_claims(at(now), 2, &mut out);
        let Ok(sweep) = sweep else {
            panic!("time is monotonic")
        };
        reaped.extend(out[..sweep.written].iter().map(|e| (e.id, e.attempt)));
        if sweep.complete {
            break;
        }
    }
    reaped.sort_unstable();
    let mut want = Vec::new();
    for i in 0..model.rows.len() {
        let row = model.rows[i];
        if row.state == State::Claimed && row.lease <= now {
            want.push((row.id, row.attempt));
            model.settle(i, now);
        }
    }
    want.sort_unstable();
    assert_eq!(reaped, want);
}
