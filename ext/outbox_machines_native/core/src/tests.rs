use super::*;
use core::assert_matches;
use core::num::NonZeroU32;
use matryona_machines::testing::refuses;
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

/// Enqueue every id at `now`, each one admitted.
fn enqueue_all<const N: usize>(outbox: &mut Outbox<N>, now: u64, ids: &[u64]) {
    for &id in ids {
        assert_eq!(
            outbox.enqueue(id, at(now)),
            Ok(Enqueue::Enqueued),
            "id {id}"
        );
    }
}

/// One `expire_claims` call into a default buffer of `K`; the sweep and the
/// entries it wrote.
fn reap<const N: usize, const K: usize>(
    outbox: &mut Outbox<N>,
    now: u64,
    max_scan: usize,
) -> (Sweep, Vec<Expired>) {
    let mut out = [Expired::default(); K];
    match outbox.expire_claims(at(now), max_scan, &mut out) {
        Ok(sweep) => (sweep, out[..sweep.written()].to_vec()),
        Err(refusal) => panic!("expected a sweep, got {refusal:?}"),
    }
}

#[test]
fn happy_path_confirms_exactly_once() {
    let mut outbox = Outbox::<2>::new(max(3));
    assert_eq!(outbox.enqueue(7, at(0)), Ok(Enqueue::Enqueued));
    let claimed = claim(&mut outbox, 0, 10);
    assert_eq!(Claimed::new(7, 1, at(10)), Some(claimed));
    assert_eq!(outbox.confirm(7, 1), Ok(Confirmation::Confirmed));
    assert_eq!(outbox.confirm(7, 1), Ok(Confirmation::AlreadyConfirmed));
    assert_eq!(outbox.get(7).map(|e| e.state()), Some(State::Confirmed));
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
    assert_eq!(
        outbox.confirm(c.id(), c.attempt()),
        Ok(Confirmation::Confirmed)
    );
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
        outbox.fail(1, c.attempt(), at(0), at(0)),
        Ok(Failure::Failed(Disposition::Dead))
    );
    assert_eq!(outbox.forget(1), Ok(Forgetting::Forgotten));
    assert!(outbox.is_empty());
    assert_eq!(outbox.forget(1), Ok(Forgetting::NotHeld));
}

#[test]
fn claims_are_fifo_among_due_records() {
    let mut outbox = Outbox::<3>::new(max(3));
    enqueue_all(&mut outbox, 0, &[30, 10, 20]);
    let order: Vec<u64> = (0..3).map(|_| claim(&mut outbox, 0, 5).id()).collect();
    assert_eq!(order, [30, 10, 20]);
    // Forgetting the oldest keeps the others ahead of a newcomer, which takes
    // the freed slot but not its place in line.
    let first = outbox.get(30).and_then(|e| e.attempt()).unwrap_or(0);
    assert_eq!(outbox.confirm(30, first), Ok(Confirmation::Confirmed));
    assert_eq!(outbox.forget(30), Ok(Forgetting::Forgotten));
    for id in [10, 20] {
        let attempt = outbox.get(id).and_then(|e| e.attempt()).unwrap_or(0);
        assert_eq!(
            outbox.fail(id, attempt, at(1), at(0)),
            Ok(Failure::Failed(Disposition::Retry(at(1))))
        );
    }
    enqueue_all(&mut outbox, 1, &[40]);
    let order: Vec<u64> = (0..3).map(|_| claim(&mut outbox, 1, 5).id()).collect();
    assert_eq!(order, [10, 20, 40]);
}

#[test]
fn fail_retries_after_delay_then_dies_at_max_attempts() {
    let mut outbox = Outbox::<1>::new(max(2));
    assert_eq!(outbox.enqueue(1, at(0)), Ok(Enqueue::Enqueued));
    let first = claim(&mut outbox, 0, 10);
    assert_eq!(
        outbox.fail(1, first.attempt(), at(1), at(50)),
        Ok(Failure::Failed(Disposition::Retry(at(51))))
    );
    assert_eq!(outbox.claim(at(50), at(10)), Ok(None));
    let second = claim(&mut outbox, 51, 10);
    assert_eq!(second.attempt(), 2);
    let entry = outbox.get(1).map(|e| (e.attempts(), e.attempt()));
    assert_eq!(entry, Some((2, Some(2))));
    assert_eq!(
        outbox.fail(1, second.attempt(), at(52), at(50)),
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
    assert_eq!(reap::<1, 1>(&mut outbox, 10, 8).0.written(), 1);
    let fresh = claim(&mut outbox, 10, 10);
    assert_eq!(fresh.attempt(), 2);
    assert_eq!(outbox.confirm(1, old.attempt()), Err(Refusal::Stale));
    assert_eq!(
        outbox.fail(1, old.attempt(), at(10), at(0)),
        Err(Refusal::Stale)
    );
    assert_eq!(outbox.confirm(2, 1), Err(Refusal::Unknown));
    assert_eq!(
        outbox.confirm(1, fresh.attempt()),
        Ok(Confirmation::Confirmed)
    );
}

#[test]
fn late_confirm_is_accepted_until_the_lease_is_reaped() {
    let mut outbox = Outbox::<3>::new(max(2));
    enqueue_all(&mut outbox, 0, &[1, 2]);
    let kept = claim(&mut outbox, 0, 5);
    let lost = claim(&mut outbox, 0, 5);
    // The clock moves far past both deadlines without a sweep.
    assert_eq!(outbox.enqueue(3, at(100)), Ok(Enqueue::Enqueued));
    assert_eq!(
        outbox.confirm(kept.id(), kept.attempt()),
        Ok(Confirmation::Confirmed)
    );
    let (sweep, reaped) = reap::<3, 3>(&mut outbox, 100, 3);
    assert_eq!(sweep.pass(), Pass::Complete);
    let retried = Expired::new(lost.id(), lost.attempt(), Disposition::Retry(at(100)));
    assert_eq!(reaped.first().copied(), retried);
    assert_eq!(reaped.len(), 1);
    // Once reaped the attempt is superseded, even before a new claim.
    assert_eq!(
        outbox.confirm(lost.id(), lost.attempt()),
        Err(Refusal::Stale)
    );
    assert_eq!(
        outbox.get(lost.id()).map(|e| e.state()),
        Some(State::Pending)
    );
}

#[test]
fn refusals_leave_the_table_untouched() {
    let mut outbox = Outbox::<2>::new(max(2));
    assert_eq!(outbox.enqueue(u64::MAX, at(10)), Ok(Enqueue::Enqueued));
    assert_eq!(outbox.enqueue(0, at(10)), Ok(Enqueue::Enqueued));
    let c = claim(&mut outbox, 10, 5);
    assert_eq!(c.id(), u64::MAX);
    let mut out = [];
    refuses(&mut outbox, Refusal::TimeRegression, |outbox| {
        outbox.enqueue(5, at(9))
    });
    refuses(&mut outbox, Refusal::TimeRegression, |outbox| {
        outbox.enqueue(0, at(9))
    });
    refuses(&mut outbox, Refusal::Full, |outbox| {
        outbox.enqueue(5, at(99))
    });
    refuses(&mut outbox, Refusal::TimeRegression, |outbox| {
        outbox.claim(at(9), at(1))
    });
    refuses(&mut outbox, Refusal::InvalidDuration, |outbox| {
        outbox.claim(at(10), Duration::ZERO)
    });
    refuses(&mut outbox, Refusal::InvalidDuration, |outbox| {
        outbox.claim(at(10), Duration::MAX)
    });
    refuses(&mut outbox, Refusal::InvalidDuration, |outbox| {
        outbox.claim(Duration::MAX, at(1))
    });
    refuses(&mut outbox, Refusal::Unknown, |outbox| outbox.confirm(7, 1));
    refuses(&mut outbox, Refusal::Stale, |outbox| {
        outbox.confirm(u64::MAX, 2)
    });
    refuses(&mut outbox, Refusal::Stale, |outbox| {
        outbox.confirm(u64::MAX, 0)
    });
    refuses(&mut outbox, Refusal::Stale, |outbox| outbox.confirm(0, 0));
    refuses(&mut outbox, Refusal::Stale, |outbox| outbox.confirm(0, 1));
    refuses(&mut outbox, Refusal::Unknown, |outbox| {
        outbox.fail(7, 1, at(10), at(0))
    });
    refuses(&mut outbox, Refusal::Stale, |outbox| {
        outbox.fail(u64::MAX, u64::MAX, at(10), at(0))
    });
    refuses(&mut outbox, Refusal::TimeRegression, |outbox| {
        outbox.fail(u64::MAX, 1, at(9), at(0))
    });
    refuses(&mut outbox, Refusal::InvalidDuration, |outbox| {
        outbox.fail(u64::MAX, 1, at(10), Duration::MAX)
    });
    refuses(&mut outbox, Refusal::InvalidDuration, |outbox| {
        outbox.fail(u64::MAX, 1, Duration::MAX, at(1))
    });
    refuses(&mut outbox, Refusal::TimeRegression, |outbox| {
        outbox.expire_claims(at(9), 2, &mut out)
    });
    refuses(&mut outbox, Refusal::NotTerminal, |outbox| outbox.forget(0));
    refuses(&mut outbox, Refusal::NotTerminal, |outbox| {
        outbox.forget(u64::MAX)
    });
}

#[test]
fn expire_claims_is_bounded_and_resumable() {
    let mut outbox = Outbox::<4>::new(max(1));
    for id in 0..4 {
        enqueue_all(&mut outbox, 0, &[id]);
        claim(&mut outbox, 0, 10);
    }
    let mut seen = Vec::new();
    for _ in 0..20 {
        let (sweep, reaped) = reap::<4, 1>(&mut outbox, 10, 8);
        seen.extend(reaped.iter().map(Expired::id));
        if sweep.pass() == Pass::Complete {
            break;
        }
    }
    seen.sort_unstable();
    assert_eq!(seen, [0, 1, 2, 3]);
    assert_eq!(outbox.count(State::Dead), 4);
    // A scan budget of zero makes no progress and never reports completion.
    let idle = reap::<4, 1>(&mut outbox, 10, 0).0;
    assert_eq!(Sweep::new(0, 0, Pass::Partial), Some(idle));
    // Zero-capacity tables complete trivially.
    let mut empty = Outbox::<0>::new(max(1));
    assert_eq!(reap::<0, 1>(&mut empty, 0, 1).0.pass(), Pass::Complete);
}

mod clock;
mod model;
mod reissue;
mod repeat;
mod scope;
mod values;

#[test]
fn a_default_buffer_is_filled_from_the_front() {
    let mut outbox = Outbox::<2>::new(max(1));
    enqueue_all(&mut outbox, 0, &[9]);
    let lost = claim(&mut outbox, 0, 5);
    let mut out = [Expired::default(); 3];
    let sweep = outbox.expire_claims(at(5), 2, &mut out);
    assert_matches!(sweep, Ok(s) if s.written() == 1);
    let reaped = Expired::new(lost.id(), lost.attempt(), Disposition::Dead);
    assert_eq!(Some(out[0]), reaped);
    assert_eq!(out[1..], [Expired::default(); 2]);
}
