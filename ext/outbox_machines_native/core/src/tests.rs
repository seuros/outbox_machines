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
    assert_eq!((claimed.id, claimed.attempt), (7, 1));
    assert_eq!(claimed.lease_expires, at(10));
    assert_eq!(outbox.confirm(7, 1), Ok(Confirmation::Confirmed));
    assert_eq!(outbox.confirm(7, 1), Ok(Confirmation::AlreadyConfirmed));
    assert_eq!(outbox.get(7).map(|e| e.state), Some(State::Confirmed));
    assert_eq!(outbox.claim(at(1), at(10)), Ok(None));
}

#[test]
fn enqueue_refuses_duplicates_and_overflow_without_evicting() {
    let mut outbox = Outbox::<1>::new(max(1));
    assert_eq!(outbox.enqueue(1, at(0)), Ok(Enqueue::Enqueued));
    assert_eq!(outbox.enqueue(1, at(0)), Ok(Enqueue::Duplicate));
    assert_eq!(outbox.enqueue(2, at(0)), Err(Refusal::Full));
    assert_eq!(outbox.len(), 1);
    assert_eq!(
        Outbox::<0>::new(max(1)).enqueue(1, at(0)),
        Err(Refusal::Full)
    );
    // Terminal records keep their id until forgotten.
    let c = claim(&mut outbox, 0, 5);
    assert_eq!(outbox.confirm(c.id, c.attempt), Ok(Confirmation::Confirmed));
    assert_eq!(outbox.enqueue(1, at(1)), Ok(Enqueue::Duplicate));
    assert_eq!(outbox.forget(1), Ok(Forgetting::Forgotten));
    assert_eq!(outbox.enqueue(1, at(1)), Ok(Enqueue::Enqueued));
}

#[test]
fn forget_only_drops_terminal_records() {
    let mut outbox = Outbox::<2>::new(max(1));
    assert_eq!(outbox.forget(1), Ok(Forgetting::NotHeld));
    assert_eq!(outbox.enqueue(1, at(0)), Ok(Enqueue::Enqueued));
    assert_eq!(outbox.forget(1), Err(Refusal::NotTerminal));
    let c = claim(&mut outbox, 0, 5);
    assert_eq!(outbox.forget(1), Err(Refusal::NotTerminal));
    assert_eq!(
        outbox.fail(1, c.attempt, at(0), at(0)),
        Ok(Failure::Failed(Disposition::Dead))
    );
    assert_eq!(outbox.forget(1), Ok(Forgetting::Forgotten));
    assert!(outbox.is_empty());
    assert_eq!(outbox.forget(1), Ok(Forgetting::NotHeld));
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
        outbox.fail(1, first.attempt, at(1), at(50)),
        Ok(Failure::Failed(Disposition::Retry(at(51))))
    );
    assert_eq!(outbox.claim(at(50), at(10)), Ok(None));
    let second = claim(&mut outbox, 51, 10);
    assert_eq!(second.attempt, 2);
    let entry = outbox.get(1).map(|e| (e.attempts, e.attempt));
    assert_eq!(entry, Some((2, Some(2))));
    assert_eq!(
        outbox.fail(1, second.attempt, at(52), at(50)),
        Ok(Failure::Failed(Disposition::Dead))
    );
    assert_eq!(outbox.count(State::Dead), 1);
    assert_eq!(outbox.claim(at(1_000), at(10)), Ok(None));
}

#[test]
fn stale_attempts_cannot_touch_a_newer_attempt() {
    let mut outbox = Outbox::<1>::new(max(5));
    assert_eq!(outbox.enqueue(1, at(0)), Ok(Enqueue::Enqueued));
    let old = claim(&mut outbox, 0, 10);
    let mut out = [Expired {
        id: 0,
        attempt: 0,
        disposition: Disposition::Dead,
    }; 1];
    let sweep = outbox.expire_claims(at(10), 8, &mut out);
    assert_eq!(sweep.map(|s| s.written), Ok(1));
    let fresh = claim(&mut outbox, 10, 10);
    assert_eq!(fresh.attempt, 2);
    assert_eq!(outbox.confirm(1, old.attempt), Err(Refusal::Stale));
    assert_eq!(
        outbox.fail(1, old.attempt, at(10), at(0)),
        Err(Refusal::Stale)
    );
    assert_eq!(outbox.confirm(2, 1), Err(Refusal::Unknown));
    assert_eq!(
        outbox.confirm(1, fresh.attempt),
        Ok(Confirmation::Confirmed)
    );
}

#[test]
fn late_confirm_is_accepted_until_the_lease_is_reaped() {
    let blank = Expired {
        id: 0,
        attempt: 0,
        disposition: Disposition::Dead,
    };
    let mut outbox = Outbox::<3>::new(max(2));
    assert_eq!(outbox.enqueue(1, at(0)), Ok(Enqueue::Enqueued));
    assert_eq!(outbox.enqueue(2, at(0)), Ok(Enqueue::Enqueued));
    let kept = claim(&mut outbox, 0, 5);
    let lost = claim(&mut outbox, 0, 5);
    // The clock moves far past both deadlines without a sweep.
    assert_eq!(outbox.enqueue(3, at(100)), Ok(Enqueue::Enqueued));
    assert_eq!(
        outbox.confirm(kept.id, kept.attempt),
        Ok(Confirmation::Confirmed)
    );
    let mut out = [blank; 3];
    let sweep = outbox.expire_claims(at(100), 3, &mut out);
    assert_eq!(sweep.map(|s| (s.written, s.complete)), Ok((1, true)));
    assert_eq!(
        out[0],
        Expired {
            id: lost.id,
            attempt: lost.attempt,
            disposition: Disposition::Retry(at(100)),
        }
    );
    // Once reaped the attempt is superseded, even before a new claim.
    assert_eq!(outbox.confirm(lost.id, lost.attempt), Err(Refusal::Stale));
    assert_eq!(outbox.get(lost.id).map(|e| e.state), Some(State::Pending));
}

#[test]
fn refusals_leave_the_table_untouched() {
    let mut outbox = Outbox::<2>::new(max(2));
    assert_eq!(outbox.enqueue(u64::MAX, at(10)), Ok(Enqueue::Enqueued));
    assert_eq!(outbox.enqueue(0, at(10)), Ok(Enqueue::Enqueued));
    let c = claim(&mut outbox, 10, 5);
    assert_eq!(c.id, u64::MAX);
    let before = outbox.clone();
    let mut out = [];
    assert_eq!(outbox.enqueue(5, at(9)), Err(Refusal::TimeRegression));
    assert_eq!(outbox.enqueue(0, at(9)), Err(Refusal::TimeRegression));
    assert_eq!(outbox.enqueue(5, at(99)), Err(Refusal::Full));
    assert_eq!(outbox.claim(at(9), at(1)), Err(Refusal::TimeRegression));
    assert_eq!(
        outbox.claim(at(10), Duration::ZERO),
        Err(Refusal::InvalidDuration)
    );
    assert_eq!(
        outbox.claim(at(10), Duration::MAX),
        Err(Refusal::InvalidDuration)
    );
    assert_eq!(outbox.confirm(7, 1), Err(Refusal::Unknown));
    assert_eq!(outbox.confirm(u64::MAX, 2), Err(Refusal::Stale));
    assert_eq!(outbox.confirm(u64::MAX, 0), Err(Refusal::Stale));
    assert_eq!(outbox.confirm(0, 0), Err(Refusal::Stale));
    assert_eq!(outbox.confirm(0, 1), Err(Refusal::Stale));
    assert_eq!(outbox.fail(7, 1, at(10), at(0)), Err(Refusal::Unknown));
    assert_eq!(
        outbox.fail(u64::MAX, u64::MAX, at(10), at(0)),
        Err(Refusal::Stale)
    );
    assert_eq!(
        outbox.fail(u64::MAX, 1, at(9), at(0)),
        Err(Refusal::TimeRegression)
    );
    assert_eq!(
        outbox.fail(u64::MAX, 1, at(10), Duration::MAX),
        Err(Refusal::InvalidDuration)
    );
    assert_eq!(
        outbox.expire_claims(at(9), 2, &mut out),
        Err(Refusal::TimeRegression)
    );
    assert_eq!(outbox.forget(0), Err(Refusal::NotTerminal));
    assert_eq!(outbox.forget(u64::MAX), Err(Refusal::NotTerminal));
    assert_eq!(outbox, before);
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
        attempt: 0,
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
    assert_eq!(outbox.enqueue(2, at(9)), Err(Refusal::TimeRegression));
    assert_eq!(outbox.claim(at(9), at(1)), Err(Refusal::TimeRegression));
    assert_eq!(
        outbox.claim(at(10), Duration::ZERO),
        Err(Refusal::InvalidDuration)
    );
    assert_eq!(
        outbox.claim(Duration::MAX, at(1)),
        Err(Refusal::InvalidDuration)
    );
    assert_eq!(outbox.get(1).map(|e| e.state), Some(State::Pending));
    let c = claim(&mut outbox, 10, 5);
    assert_eq!(
        outbox.fail(1, c.attempt, at(9), at(0)),
        Err(Refusal::TimeRegression)
    );
    assert_eq!(
        outbox.fail(1, c.attempt, Duration::MAX, at(1)),
        Err(Refusal::InvalidDuration)
    );
    assert_eq!(outbox.get(1).map(|e| e.state), Some(State::Claimed));
    let mut out = [];
    assert_eq!(
        outbox.expire_claims(at(9), 1, &mut out),
        Err(Refusal::TimeRegression)
    );
}

#[test]
fn sequence_exhaustion_is_refused() {
    let mut outbox = Outbox::<1>::new(max(1));
    outbox.next_seq = u64::MAX;
    let before = outbox.clone();
    assert_eq!(outbox.enqueue(1, at(7)), Err(Refusal::Exhausted));
    assert_eq!(outbox, before);
}

#[test]
fn attempt_exhaustion_is_refused_only_when_a_record_is_due() {
    let mut outbox = Outbox::<2>::new(max(3));
    assert_eq!(outbox.enqueue(1, at(0)), Ok(Enqueue::Enqueued));
    assert_eq!(outbox.enqueue(2, at(9)), Ok(Enqueue::Enqueued));
    outbox.next_attempt = u64::MAX - 1;
    let last = claim(&mut outbox, 9, 5);
    assert_eq!((last.id, last.attempt), (1, u64::MAX - 1));
    let before = outbox.clone();
    // The instant is not recorded either: the call changed nothing.
    assert_eq!(outbox.claim(at(50), at(5)), Err(Refusal::Exhausted));
    assert_eq!(outbox, before);
    assert_eq!(outbox.get(2).map(|e| e.attempt), Some(None));
    // The attempt already issued is still settled normally.
    assert_eq!(outbox.confirm(1, last.attempt), Ok(Confirmation::Confirmed));
    assert_eq!(outbox.forget(1), Ok(Forgetting::Forgotten));
    // With nothing due there is nothing to stamp, so the answer is honoured.
    let mut spent = Outbox::<1>::new(max(1));
    spent.next_attempt = u64::MAX;
    assert_eq!(spent.claim(at(3), at(5)), Ok(None));
    assert_eq!(spent.enqueue(1, at(2)), Err(Refusal::TimeRegression));
}

mod clock;
mod model;
mod reissue;
mod repeat;

#[test]
fn a_default_buffer_is_filled_from_the_front() {
    let mut outbox = Outbox::<2>::new(max(1));
    assert_eq!(outbox.enqueue(9, at(0)), Ok(Enqueue::Enqueued));
    let lost = claim(&mut outbox, 0, 5);
    let mut out = [Expired::default(); 3];
    assert_eq!(out[0].attempt, 0);
    assert_ne!(lost.attempt, 0);
    let sweep = outbox.expire_claims(at(5), 2, &mut out);
    assert_eq!(sweep.map(|s| s.written), Ok(1));
    let reaped = Expired {
        id: lost.id,
        attempt: lost.attempt,
        disposition: Disposition::Dead,
    };
    assert_eq!(out[0], reaped);
    assert_eq!(out[1..], [Expired::default(); 2]);
}
