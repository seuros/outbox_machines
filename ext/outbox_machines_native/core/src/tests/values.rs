//! The validating constructors answer `None` for a value no outbox answers,
//! and their accessors read back what was built.

use super::*;
use periscope_machines::scoped;

#[test]
fn a_claimed_needs_an_issuable_attempt_and_a_lease_end() {
    let claimed = Claimed::new(7, scoped(9, 3), at(10));
    assert_eq!(
        claimed.map(|c| (c.id(), c.attempt(), c.lease_expires(), c.scope())),
        Some((7, scoped(9, 3), at(10), 9))
    );
    for (attempt, lease_expires) in [(0, at(10)), (scoped(9, 0), at(10)), (1, Duration::ZERO)] {
        assert_eq!(Claimed::new(7, attempt, lease_expires), None);
    }
}

#[test]
fn an_expired_needs_an_issuable_attempt() {
    let expired = Expired::new(7, scoped(4, 2), Disposition::Dead);
    assert_eq!(
        expired.map(|e| (e.id(), e.attempt(), e.disposition(), e.scope())),
        Some((7, scoped(4, 2), Disposition::Dead, 4))
    );
    for attempt in [0, scoped(4, 0)] {
        assert_eq!(Expired::new(7, attempt, Disposition::Retry(at(1))), None);
    }
    // The placeholder is reachable through `default` only.
    assert_eq!(Expired::default().attempt(), 0);
}

#[test]
fn a_sweep_writes_no_more_than_it_scanned() {
    let sweep = Sweep::new(2, 3, Pass::Partial);
    assert_eq!(
        sweep.map(|s| (s.written(), s.scanned(), s.pass())),
        Some((2, 3, Pass::Partial))
    );
    for (written, scanned, valid) in [(0, 0, true), (3, 3, true), (4, 3, false)] {
        assert_eq!(
            Sweep::new(written, scanned, Pass::Complete).is_some(),
            valid
        );
    }
}

#[test]
fn an_entry_agrees_with_itself() {
    let entry = Entry::new(1, State::Pending, 2, Some(5), at(3));
    assert_eq!(
        entry.map(|e| (
            e.id(),
            e.state(),
            e.attempts(),
            e.attempt(),
            e.available_at()
        )),
        Some((1, State::Pending, 2, Some(5), at(3)))
    );
    assert!(Entry::new(1, State::Pending, 0, None, at(3)).is_some());
    for state in [State::Claimed, State::Confirmed, State::Dead] {
        assert!(Entry::new(1, state, 1, Some(1), at(3)).is_some());
        // Only a pending record may be unclaimed.
        assert_eq!(Entry::new(1, state, 0, None, at(3)), None);
    }
    // An attempt and a claim count come together; sequence zero is no attempt.
    let refused = [
        (State::Pending, 1, None),
        (State::Pending, 0, Some(1)),
        (State::Claimed, 1, Some(0)),
        (State::Claimed, 1, Some(scoped(2, 0))),
    ];
    for (state, attempts, attempt) in refused {
        assert_eq!(Entry::new(1, state, attempts, attempt, at(3)), None);
    }
}
