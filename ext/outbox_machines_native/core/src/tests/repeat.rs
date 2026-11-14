//! An idempotent repeat is honoured and changes nothing, not even the clock.

use super::*;

#[test]
fn a_repeated_confirm_is_honoured_and_changes_nothing() {
    let mut outbox = Outbox::<2>::new(max(3));
    assert_eq!(outbox.enqueue(1, at(0)), Ok(Enqueue::Enqueued));
    let first = claim(&mut outbox, 0, 10);
    assert_eq!(
        outbox.fail(1, first.attempt(), at(1), at(0)),
        Ok(Failure::Failed(Disposition::Retry(at(1))))
    );
    let second = claim(&mut outbox, 1, 10);
    assert_eq!(
        outbox.confirm(1, second.attempt()),
        Ok(Confirmation::Confirmed)
    );
    assert_eq!(
        unchanged(&mut outbox, |o| o.confirm(1, second.attempt())),
        Ok(Confirmation::AlreadyConfirmed)
    );
    // Only the confirming attempt repeats; a superseded one stays stale.
    refuses(&mut outbox, Refusal::Stale, |o| {
        o.confirm(1, first.attempt())
    });
    refuses(&mut outbox, Refusal::Stale, |o| {
        o.confirm(1, second.attempt() + 1)
    });
    refuses(&mut outbox, Refusal::Stale, |o| o.confirm(1, 0));
    // A confirmed attempt was not failed.
    refuses(&mut outbox, Refusal::Stale, |o| {
        o.fail(1, second.attempt(), at(1), at(0))
    });
    // Once forgotten the proof is gone with the record.
    assert_eq!(outbox.forget(1), Ok(Forgetting::Forgotten));
    assert_eq!(outbox.confirm(1, second.attempt()), Err(Refusal::Unknown));
}

#[test]
fn a_repeated_fail_answers_the_standing_disposition() {
    let mut outbox = Outbox::<2>::new(max(2));
    assert_eq!(outbox.enqueue(1, at(0)), Ok(Enqueue::Enqueued));
    let first = claim(&mut outbox, 0, 10);
    let retry = Disposition::Retry(at(55));
    assert_eq!(
        outbox.fail(1, first.attempt(), at(5), at(50)),
        Ok(Failure::Failed(retry))
    );
    assert_eq!(
        unchanged(&mut outbox, |o| o.fail(1, first.attempt(), at(5), at(50))),
        Ok(Failure::AlreadyFailed(retry))
    );
    // A later instant is the same report made later: honoured, with the
    // standing disposition, and the instant is not recorded as observed.
    assert_eq!(
        unchanged(&mut outbox, |o| o.fail(1, first.attempt(), at(900), at(50))),
        Ok(Failure::AlreadyFailed(retry))
    );
    // Another delay is another report, even one landing on the same instant.
    refuses(&mut outbox, Refusal::Mismatch, |o| {
        o.fail(1, first.attempt(), at(900), at(7))
    });
    refuses(&mut outbox, Refusal::Mismatch, |o| {
        o.fail(1, first.attempt(), at(6), at(49))
    });
    // The arguments of a repeat are still checked, before the payload is.
    refuses(&mut outbox, Refusal::TimeRegression, |o| {
        o.fail(1, first.attempt(), at(4), at(50))
    });
    refuses(&mut outbox, Refusal::TimeRegression, |o| {
        o.fail(1, first.attempt(), at(4), at(7))
    });
    refuses(&mut outbox, Refusal::InvalidDuration, |o| {
        o.fail(1, first.attempt(), at(5), Duration::MAX)
    });
    // A failed attempt cannot be confirmed afterwards.
    refuses(&mut outbox, Refusal::Stale, |o| {
        o.confirm(1, first.attempt())
    });
    assert_eq!(outbox.claim(at(54), at(10)), Ok(None));

    // A new claim supersedes the failed attempt.
    let second = claim(&mut outbox, 55, 10);
    assert_eq!(
        outbox.fail(1, first.attempt(), at(55), at(0)),
        Err(Refusal::Stale)
    );
    assert_eq!(
        outbox.fail(1, second.attempt(), at(56), at(0)),
        Ok(Failure::Failed(Disposition::Dead))
    );
    assert_eq!(
        unchanged(&mut outbox, |o| o.fail(1, second.attempt(), at(999), at(0))),
        Ok(Failure::AlreadyFailed(Disposition::Dead))
    );
    // The delay did not shape a dead record, but it is still what was said.
    refuses(&mut outbox, Refusal::Mismatch, |o| {
        o.fail(1, second.attempt(), at(999), at(3))
    });
}

#[test]
fn a_reaped_attempt_was_not_failed() {
    let mut outbox = Outbox::<2>::new(max(1));
    assert_eq!(outbox.enqueue(1, at(0)), Ok(Enqueue::Enqueued));
    assert_eq!(outbox.enqueue(2, at(0)), Ok(Enqueue::Enqueued));
    let retried = claim(&mut outbox, 0, 5);
    let mut out = [Expired::default(); 2];
    let sweep = outbox.expire_claims(at(5), 2, &mut out);
    assert_eq!(sweep.map(|s| s.written()), Ok(1));
    assert_eq!(out[0].disposition(), Disposition::Dead);
    // Never claimed: attempt 0 names no attempt.
    refuses(&mut outbox, Refusal::Stale, |o| o.fail(2, 0, at(5), at(0)));
    // Expiry is an unknown outcome; the outbox cannot call it a failure.
    refuses(&mut outbox, Refusal::Stale, |o| {
        o.fail(1, retried.attempt(), at(5), at(0))
    });
}

#[test]
fn forgetting_an_absent_id_is_honoured_and_changes_nothing() {
    let mut outbox = Outbox::<2>::new(max(1));
    assert_eq!(outbox.enqueue(1, at(3)), Ok(Enqueue::Enqueued));
    let c = claim(&mut outbox, 3, 5);
    assert_eq!(outbox.confirm(1, c.attempt()), Ok(Confirmation::Confirmed));
    assert_eq!(
        unchanged(&mut outbox, |o| o.forget(9)),
        Ok(Forgetting::NotHeld)
    );
    assert_eq!(outbox.forget(1), Ok(Forgetting::Forgotten));
    assert_eq!(
        unchanged(&mut outbox, |o| o.forget(1)),
        Ok(Forgetting::NotHeld)
    );
    assert_eq!(Outbox::<0>::new(max(1)).forget(1), Ok(Forgetting::NotHeld));
}
