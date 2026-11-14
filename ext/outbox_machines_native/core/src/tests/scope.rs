//! Attempt identities carry the scope of the outbox that issued them.

use super::*;
use periscope_machines::{SEQUENCE_MAX, scoped};

/// An outbox of `scope` that has already issued `issued` attempt identities.
fn resumed<const N: usize>(max_attempts: u32, scope: u16, issued: u64) -> Outbox<N> {
    Outbox::resumed(max(max_attempts), scope, issued)
        .unwrap_or_else(|| panic!("{issued} is past the sequence"))
}

/// Enqueue ids 7 and 8, confirm the claim on 7, fail the claim on 8.
fn settled(scope: u16) -> (Outbox<3>, Claimed, Claimed) {
    let mut outbox = Outbox::<3>::with_scope(max(3), scope);
    assert_eq!(outbox.enqueue(7, at(0)), Ok(Enqueue::Enqueued));
    assert_eq!(outbox.enqueue(8, at(0)), Ok(Enqueue::Enqueued));
    let confirmed = claim(&mut outbox, 0, 10);
    let failed = claim(&mut outbox, 0, 10);
    assert_eq!(
        outbox.confirm(7, confirmed.attempt()),
        Ok(Confirmation::Confirmed)
    );
    assert_eq!(
        outbox.fail(8, failed.attempt(), at(1), at(4)),
        Ok(Failure::Failed(Disposition::Retry(at(5))))
    );
    (outbox, confirmed, failed)
}

#[test]
fn an_attempt_from_another_outbox_settles_nothing() {
    // The review's sequence: two outboxes, the same record ids, the same calls.
    let (mut near, near_confirmed, near_failed) = settled(1);
    let (mut far, far_confirmed, far_failed) = settled(2);
    assert_eq!(near.enqueue(9, at(1)), Ok(Enqueue::Enqueued));
    assert_eq!(far.enqueue(9, at(1)), Ok(Enqueue::Enqueued));
    let near_live = claim(&mut near, 1, 10);
    let far_live = claim(&mut far, 1, 10);
    assert_eq!((near_live.id(), far_live.id()), (9, 9));
    assert_ne!(near_live.attempt(), far_live.attempt());
    assert_eq!((near_live.scope(), far_live.scope()), (1, 2));

    let foreign = [
        (&mut near, [far_confirmed, far_failed, far_live]),
        (&mut far, [near_confirmed, near_failed, near_live]),
    ];
    for (outbox, [confirmed, failed, live]) in foreign {
        let before = outbox.clone();
        // A live claim on the same id is not settled.
        assert_eq!(outbox.confirm(9, live.attempt()), Err(Refusal::Stale));
        assert_eq!(
            outbox.fail(9, live.attempt(), at(1), at(4)),
            Err(Refusal::Stale)
        );
        // Nor is a repeat honoured: the confirmed and failed attempts here
        // carry the same sequence numbers, but they are not these.
        assert_eq!(outbox.confirm(7, confirmed.attempt()), Err(Refusal::Stale));
        assert_eq!(
            outbox.fail(7, confirmed.attempt(), at(1), at(4)),
            Err(Refusal::Stale)
        );
        assert_eq!(
            outbox.fail(8, failed.attempt(), at(1), at(4)),
            Err(Refusal::Stale)
        );
        assert_eq!(outbox.confirm(8, failed.attempt()), Err(Refusal::Stale));
        // Refused before any record is looked up: the id does not matter.
        assert_eq!(outbox.confirm(404, live.attempt()), Err(Refusal::Stale));
        assert_eq!(
            outbox.fail(404, live.attempt(), at(1), at(4)),
            Err(Refusal::Stale)
        );
        assert_eq!(*outbox, before);
    }

    // Each outbox still honours its own.
    assert_eq!(
        near.confirm(9, near_live.attempt()),
        Ok(Confirmation::Confirmed)
    );
    assert_eq!(
        near.confirm(7, near_confirmed.attempt()),
        Ok(Confirmation::AlreadyConfirmed)
    );
    assert_eq!(
        far.fail(8, far_failed.attempt(), at(1), at(4)),
        Ok(Failure::AlreadyFailed(Disposition::Retry(at(5))))
    );
}

#[test]
fn two_outboxes_left_in_one_scope_still_alias() {
    // Nothing tells them apart: keeping scopes distinct is the caller's job.
    let (mut one, _, _) = settled(5);
    let (mut other, other_confirmed, _) = settled(5);
    assert_eq!(one.enqueue(9, at(1)), Ok(Enqueue::Enqueued));
    assert_eq!(other.enqueue(9, at(1)), Ok(Enqueue::Enqueued));
    let mine = claim(&mut one, 1, 10);
    let theirs = claim(&mut other, 1, 10);
    assert_eq!(mine, theirs);
    assert_eq!(
        one.confirm(9, theirs.attempt()),
        Ok(Confirmation::Confirmed)
    );
    assert_eq!(
        one.confirm(7, other_confirmed.attempt()),
        Ok(Confirmation::AlreadyConfirmed)
    );
}

#[test]
fn scope_zero_issues_the_plain_counter() {
    let mut outbox = Outbox::<2>::new(max(3));
    assert_eq!(outbox.scope(), 0);
    assert_eq!(outbox, Outbox::<2>::with_scope(max(3), 0));
    assert_eq!(outbox.enqueue(1, at(0)), Ok(Enqueue::Enqueued));
    assert_eq!(outbox.enqueue(2, at(0)), Ok(Enqueue::Enqueued));
    let attempts = [claim(&mut outbox, 0, 5), claim(&mut outbox, 0, 5)].map(|c| c.attempt());
    assert_eq!(attempts, [1, 2]);

    let mut scoped_box = Outbox::<2>::with_scope(max(3), 9);
    assert_eq!(scoped_box.scope(), 9);
    assert_eq!(scoped_box.enqueue(1, at(0)), Ok(Enqueue::Enqueued));
    let first = claim(&mut scoped_box, 0, 5);
    assert_eq!((first.attempt(), first.scope()), (scoped(9, 1), 9));
    assert_eq!(
        scoped_box.get(1).and_then(|e| e.attempt()),
        Some(scoped(9, 1))
    );
    let mut out = [Expired::default(); 1];
    let sweep = scoped_box.expire_claims(at(5), 2, &mut out);
    assert_eq!(sweep.map(|s| s.written()), Ok(1));
    assert_eq!((out[0].attempt(), out[0].scope()), (scoped(9, 1), 9));
    // The plain counter of scope zero is foreign to it.
    assert_eq!(scoped_box.confirm(1, 1), Err(Refusal::Stale));
}

#[test]
fn the_last_attempt_is_issued_then_the_scope_is_spent() {
    let mut outbox = resumed::<2>(3, u16::MAX, SEQUENCE_MAX - 1);
    assert_eq!(outbox.enqueue(1, at(0)), Ok(Enqueue::Enqueued));
    assert_eq!(outbox.enqueue(2, at(9)), Ok(Enqueue::Enqueued));
    let last = claim(&mut outbox, 9, 5);
    assert_eq!((last.id(), last.attempt()), (1, u64::MAX));
    let before = outbox.clone();
    // The instant is not recorded either: the call changed nothing.
    assert_eq!(outbox.claim(at(50), at(5)), Err(Refusal::Exhausted));
    assert_eq!(outbox, before);
    assert_eq!(outbox.get(2).map(|e| e.attempt()), Some(None));
    // The attempt already issued is still settled normally.
    assert_eq!(
        outbox.confirm(1, last.attempt()),
        Ok(Confirmation::Confirmed)
    );
    assert_eq!(outbox.forget(1), Ok(Forgetting::Forgotten));
    // With nothing due there is nothing to stamp, so the answer is honoured.
    let mut spent = resumed::<1>(1, 0, SEQUENCE_MAX);
    assert_eq!(spent.issued(), SEQUENCE_MAX);
    assert_eq!(spent.claim(at(3), at(5)), Ok(None));
    assert_eq!(spent.enqueue(1, at(2)), Err(Refusal::TimeRegression));
}

#[test]
fn an_attempt_this_outbox_never_issued_is_refused_before_any_lookup() {
    let mut outbox = Outbox::<2>::with_scope(max(3), 3);
    assert_eq!(outbox.enqueue(1, at(0)), Ok(Enqueue::Enqueued));
    let issued = claim(&mut outbox, 0, 10);
    assert_eq!(issued.attempt(), scoped(3, 1));
    let before = outbox.clone();
    // In scope, but past what was issued, or the sequence zero no issuer
    // hands out: not ours, whatever the record id.
    for never in [scoped(3, 2), scoped(3, 0), scoped(3, SEQUENCE_MAX)] {
        assert_eq!(outbox.confirm(404, never), Err(Refusal::Stale));
        assert_eq!(outbox.fail(404, never, at(1), at(4)), Err(Refusal::Stale));
        assert_eq!(outbox.confirm(1, never), Err(Refusal::Stale));
        assert_eq!(outbox.fail(1, never, at(1), at(4)), Err(Refusal::Stale));
    }
    // An attempt it did issue still reaches the lookup.
    assert_eq!(outbox.confirm(404, issued.attempt()), Err(Refusal::Unknown));
    assert_eq!(
        outbox.fail(404, issued.attempt(), at(1), at(4)),
        Err(Refusal::Unknown)
    );
    assert_eq!(outbox, before);
}

#[test]
fn a_resumed_outbox_continues_the_sequence_it_saved() {
    for scope in [0, 9, u16::MAX] {
        // Nothing issued yet: resuming is starting.
        let fresh = Outbox::<2>::with_scope(max(3), scope);
        assert_eq!(fresh.issued(), 0);
        assert_eq!(Outbox::resumed(max(3), scope, 0), Some(fresh));

        let mut first = Outbox::<2>::with_scope(max(3), scope);
        assert_eq!(first.enqueue(7, at(0)), Ok(Enqueue::Enqueued));
        assert_eq!(first.enqueue(8, at(0)), Ok(Enqueue::Enqueued));
        let old = [claim(&mut first, 0, 10), claim(&mut first, 0, 10)];
        assert_eq!(first.issued(), 2);

        let mut second = resumed::<2>(3, first.scope(), first.issued());
        assert_eq!((second.scope(), second.issued()), (scope, 2));
        assert_eq!((second.len(), second.max_attempts()), (0, 3));
        assert_eq!(second.enqueue(7, at(0)), Ok(Enqueue::Enqueued));
        let new = claim(&mut second, 0, 10);
        assert_eq!(new.attempt(), scoped(scope, 3));
        assert_eq!(second.issued(), 3);
        let before = second.clone();
        // An attempt that outlived the restart is this outbox's own, and
        // names no claim made since.
        for late in old {
            assert_eq!(second.confirm(7, late.attempt()), Err(Refusal::Stale));
            assert_eq!(second.confirm(8, late.attempt()), Err(Refusal::Unknown));
        }
        assert_eq!(second.confirm(8, scoped(scope, 4)), Err(Refusal::Stale));
        assert_eq!(second, before);
        assert_eq!(
            second.confirm(7, new.attempt()),
            Ok(Confirmation::Confirmed)
        );
    }
}

#[test]
fn a_restart_that_resumes_nothing_reissues_the_old_attempts() {
    // What `resumed` and a new scope per incarnation both prevent.
    let mut first = Outbox::<1>::with_scope(max(3), 4);
    assert_eq!(first.enqueue(7, at(0)), Ok(Enqueue::Enqueued));
    let old = claim(&mut first, 0, 10);
    let mut again = Outbox::<1>::with_scope(max(3), 4);
    assert_eq!(again.enqueue(7, at(0)), Ok(Enqueue::Enqueued));
    assert_eq!(claim(&mut again, 0, 10).attempt(), old.attempt());
    assert_eq!(again.confirm(7, old.attempt()), Ok(Confirmation::Confirmed));
    // A new scope refuses the survivor instead.
    let mut moved = Outbox::<1>::with_scope(max(3), 5);
    assert_eq!(moved.enqueue(7, at(0)), Ok(Enqueue::Enqueued));
    assert_ne!(claim(&mut moved, 0, 10).attempt(), old.attempt());
    assert_eq!(moved.confirm(7, old.attempt()), Err(Refusal::Stale));
}

#[test]
fn a_count_past_the_sequence_resumes_nothing() {
    assert_eq!(Outbox::<1>::resumed(max(1), 0, SEQUENCE_MAX + 1), None);
    assert_eq!(Outbox::<1>::resumed(max(1), 7, u64::MAX), None);
    assert!(Outbox::<1>::resumed(max(1), 7, SEQUENCE_MAX).is_some());
}
