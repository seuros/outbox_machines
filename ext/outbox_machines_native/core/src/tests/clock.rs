//! The observed clock moves only when a call is honoured and takes effect.

use super::*;

#[test]
fn a_full_enqueue_changes_nothing() {
    let mut outbox = Outbox::<1>::new(max(2));
    assert_eq!(outbox.enqueue(1, at(10)), Ok(Enqueue::Enqueued));
    let before = outbox.clone();
    assert_eq!(outbox.enqueue(2, at(500)), Err(Refusal::Full));
    assert_eq!(outbox, before);
    // The refused instant was not recorded: time in between is still fine.
    assert_eq!(
        outbox.claim(at(10), at(5)).map(|c| c.map(|c| c.id)),
        Ok(Some(1))
    );
}

#[test]
fn a_duplicate_enqueue_changes_nothing() {
    let mut outbox = Outbox::<2>::new(max(2));
    assert_eq!(outbox.enqueue(1, at(10)), Ok(Enqueue::Enqueued));
    let before = outbox.clone();
    assert_eq!(outbox.enqueue(1, at(500)), Ok(Enqueue::Duplicate));
    assert_eq!(outbox, before);
    assert_eq!(outbox.get(1).map(|e| e.available_at), Some(at(10)));
    assert_eq!(outbox.enqueue(2, at(10)), Ok(Enqueue::Enqueued));
    // Backwards time is refused before the repeat is recognised.
    assert_eq!(outbox.enqueue(1, at(9)), Err(Refusal::TimeRegression));
}

#[test]
fn id_and_attempt_refusals_do_not_record_the_instant() {
    let mut outbox = Outbox::<2>::new(max(2));
    assert_eq!(outbox.enqueue(1, at(10)), Ok(Enqueue::Enqueued));
    let c = claim(&mut outbox, 10, 5);
    let before = outbox.clone();
    assert_eq!(outbox.fail(9, 1, at(500), at(0)), Err(Refusal::Unknown));
    assert_eq!(
        outbox.fail(1, c.attempt + 1, at(500), at(0)),
        Err(Refusal::Stale)
    );
    assert_eq!(
        outbox.fail(1, c.attempt, at(500), Duration::MAX),
        Err(Refusal::InvalidDuration)
    );
    assert_eq!(
        outbox.claim(at(500), Duration::ZERO),
        Err(Refusal::InvalidDuration)
    );
    assert_eq!(
        outbox.claim(at(500), Duration::MAX),
        Err(Refusal::InvalidDuration)
    );
    assert_eq!(outbox.confirm(1, c.attempt + 1), Err(Refusal::Stale));
    assert_eq!(outbox.forget(1), Err(Refusal::NotTerminal));
    assert_eq!(outbox, before);
}

#[test]
fn honoured_calls_record_the_instant_even_when_nothing_is_due() {
    let mut outbox = Outbox::<2>::new(max(2));
    assert_eq!(outbox.claim(at(20), at(5)), Ok(None));
    // "Nothing was due at 20" stays true: nothing may be enqueued before it.
    assert_eq!(outbox.enqueue(1, at(19)), Err(Refusal::TimeRegression));
    let mut out = [Expired::default(); 1];
    let sweep = outbox.expire_claims(at(30), 0, &mut out);
    assert_eq!(sweep.map(|s| (s.written, s.scanned)), Ok((0, 0)));
    assert_eq!(outbox.enqueue(1, at(29)), Err(Refusal::TimeRegression));
    assert_eq!(outbox.enqueue(1, at(30)), Ok(Enqueue::Enqueued));
    let c = claim(&mut outbox, 30, 5);
    assert_eq!(
        outbox.fail(1, c.attempt, at(40), at(1)),
        Ok(Failure::Failed(Disposition::Retry(at(41))))
    );
    assert_eq!(outbox.claim(at(39), at(5)), Err(Refusal::TimeRegression));
}

#[test]
fn refusals_display_a_reason() {
    fn is_error<E: core::error::Error>(_: &E) {}
    is_error(&Refusal::Full);
    assert_eq!(std::format!("{}", Refusal::Full), "every slot is occupied");
    assert_ne!(
        std::format!("{}", Refusal::Stale),
        std::format!("{}", Refusal::Unknown)
    );
}
