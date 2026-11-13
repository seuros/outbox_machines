//! Attempt identities are never reissued, and a repeat must match its record.

use super::*;

#[test]
fn a_report_from_a_forgotten_life_cannot_settle_the_new_one() {
    let mut outbox = Outbox::<2>::new(max(3));
    assert_eq!(outbox.enqueue(7, at(0)), Ok(Enqueue::Enqueued));
    let old = claim(&mut outbox, 0, 10);
    assert_eq!(outbox.confirm(7, old.attempt), Ok(Confirmation::Confirmed));
    assert_eq!(outbox.forget(7), Ok(Forgetting::Forgotten));
    assert_eq!(outbox.enqueue(7, at(0)), Ok(Enqueue::Enqueued));
    // Held but not claimed yet: the old identity names nothing live.
    let before = outbox.clone();
    assert_eq!(outbox.confirm(7, old.attempt), Err(Refusal::Stale));
    assert_eq!(
        outbox.fail(7, old.attempt, at(0), at(0)),
        Err(Refusal::Stale)
    );
    assert_eq!(outbox, before);
    let fresh = claim(&mut outbox, 0, 10);
    assert_ne!(fresh.attempt, old.attempt);
    // The record's own count restarted; the identity did not.
    let entry = outbox.get(7).map(|e| (e.attempts, e.attempt));
    assert_eq!(entry, Some((1, Some(fresh.attempt))));
    let before = outbox.clone();
    assert_eq!(outbox.confirm(7, old.attempt), Err(Refusal::Stale));
    assert_eq!(
        outbox.fail(7, old.attempt, at(0), at(0)),
        Err(Refusal::Stale)
    );
    assert_eq!(outbox, before);
    assert_eq!(outbox.get(7).map(|e| e.state), Some(State::Claimed));
    assert_eq!(
        outbox.confirm(7, fresh.attempt),
        Ok(Confirmation::Confirmed)
    );
}

#[test]
fn a_late_fail_from_a_dead_life_cannot_fail_the_new_one() {
    let mut outbox = Outbox::<1>::new(max(1));
    assert_eq!(outbox.enqueue(7, at(0)), Ok(Enqueue::Enqueued));
    let old = claim(&mut outbox, 0, 10);
    assert_eq!(
        outbox.fail(7, old.attempt, at(0), at(4)),
        Ok(Failure::Failed(Disposition::Dead))
    );
    assert_eq!(outbox.forget(7), Ok(Forgetting::Forgotten));
    assert_eq!(outbox.enqueue(7, at(0)), Ok(Enqueue::Enqueued));
    let fresh = claim(&mut outbox, 0, 10);
    let before = outbox.clone();
    // The same report again, word for word, is about a record that is gone.
    assert_eq!(
        outbox.fail(7, old.attempt, at(0), at(4)),
        Err(Refusal::Stale)
    );
    assert_eq!(outbox.confirm(7, old.attempt), Err(Refusal::Stale));
    assert_eq!(outbox, before);
    assert_eq!(
        outbox.fail(7, fresh.attempt, at(0), at(4)),
        Ok(Failure::Failed(Disposition::Dead))
    );
}

#[test]
fn a_repeat_is_honoured_only_for_the_attempt_that_produced_the_outcome() {
    let mut outbox = Outbox::<2>::new(max(2));
    // First life of id 7 ends confirmed, first life of id 8 ends failed.
    assert_eq!(outbox.enqueue(7, at(0)), Ok(Enqueue::Enqueued));
    assert_eq!(outbox.enqueue(8, at(0)), Ok(Enqueue::Enqueued));
    let old7 = claim(&mut outbox, 0, 10);
    let old8 = claim(&mut outbox, 0, 10);
    assert_eq!(outbox.confirm(7, old7.attempt), Ok(Confirmation::Confirmed));
    let again = claim_after_fail(&mut outbox, old8);
    assert_eq!(
        outbox.fail(8, again.attempt, at(0), at(0)),
        Ok(Failure::Failed(Disposition::Dead))
    );
    assert_eq!(outbox.forget(7), Ok(Forgetting::Forgotten));
    assert_eq!(outbox.forget(8), Ok(Forgetting::Forgotten));

    // Second lives reach the same outcomes with the same per-record counts.
    assert_eq!(outbox.enqueue(7, at(0)), Ok(Enqueue::Enqueued));
    assert_eq!(outbox.enqueue(8, at(0)), Ok(Enqueue::Enqueued));
    let new7 = claim(&mut outbox, 0, 10);
    let new8 = claim(&mut outbox, 0, 10);
    assert_eq!(outbox.confirm(7, new7.attempt), Ok(Confirmation::Confirmed));
    let retry = Disposition::Retry(at(0));
    assert_eq!(
        outbox.fail(8, new8.attempt, at(0), at(0)),
        Ok(Failure::Failed(retry))
    );
    assert_eq!(outbox.get(7).map(|e| e.attempts), Some(1));
    assert_eq!(outbox.get(8).map(|e| e.attempts), Some(1));

    let before = outbox.clone();
    assert_eq!(
        outbox.confirm(7, new7.attempt),
        Ok(Confirmation::AlreadyConfirmed)
    );
    assert_eq!(outbox.confirm(7, old7.attempt), Err(Refusal::Stale));
    assert_eq!(
        outbox.fail(8, new8.attempt, at(0), at(0)),
        Ok(Failure::AlreadyFailed(retry))
    );
    for old in [old8.attempt, again.attempt, old7.attempt, new7.attempt] {
        assert_eq!(outbox.fail(8, old, at(0), at(0)), Err(Refusal::Stale));
    }
    assert_eq!(outbox, before);
}

/// Fail the claim for immediate retry and claim the record again.
fn claim_after_fail<const N: usize>(outbox: &mut Outbox<N>, claimed: Claimed) -> Claimed {
    assert_eq!(
        outbox.fail(claimed.id, claimed.attempt, at(0), at(0)),
        Ok(Failure::Failed(Disposition::Retry(at(0))))
    );
    claim(outbox, 0, 10)
}

#[test]
fn a_repeated_fail_with_another_retry_after_is_a_mismatch() {
    let mut outbox = Outbox::<2>::new(max(3));
    assert_eq!(outbox.enqueue(1, at(0)), Ok(Enqueue::Enqueued));
    let first = claim(&mut outbox, 0, 10);
    let retry = Disposition::Retry(at(55));
    assert_eq!(
        outbox.fail(1, first.attempt, at(5), at(50)),
        Ok(Failure::Failed(retry))
    );
    let before = outbox.clone();
    assert_eq!(
        outbox.fail(1, first.attempt, at(900), at(7)),
        Err(Refusal::Mismatch)
    );
    // Even a delay that would land on the same instant is a different report.
    assert_eq!(
        outbox.fail(1, first.attempt, at(6), at(49)),
        Err(Refusal::Mismatch)
    );
    // Malformed arguments are refused before the payload is compared.
    assert_eq!(
        outbox.fail(1, first.attempt, at(4), at(7)),
        Err(Refusal::TimeRegression)
    );
    assert_eq!(
        outbox.fail(1, first.attempt, at(5), Duration::MAX),
        Err(Refusal::InvalidDuration)
    );
    // `now` is the instant of the call, not payload.
    assert_eq!(
        outbox.fail(1, first.attempt, at(900), at(50)),
        Ok(Failure::AlreadyFailed(retry))
    );
    assert_eq!(outbox, before);
    assert_eq!(outbox.get(1).map(|e| e.available_at), Some(at(55)));
}
