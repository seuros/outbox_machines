//! `Outbox::restored` rebuilds exactly the records some outbox holds, and the
//! rebuilt record answers every later report as the original does.

use super::*;
use periscope_machines::scoped;

fn stored(
    state: State,
    attempts: u32,
    latest: Option<(u64, u64)>,
    available_at: u64,
    failed: Option<u64>,
) -> Option<Stored> {
    let latest = latest.and_then(|(attempt, expires)| Claimed::new(7, attempt, at(expires)));
    Stored::new(7, state, attempts, latest, at(available_at), failed.map(at))
}

fn kept(
    state: State,
    attempts: u32,
    latest: Option<(u64, u64)>,
    available_at: u64,
    failed: Option<u64>,
) -> Stored {
    let record = stored(state, attempts, latest, available_at, failed);
    record.unwrap_or_else(|| panic!("a reachable record: {state:?} {attempts} {latest:?}"))
}

#[test]
fn a_stored_record_holds_together() {
    let record = kept(State::Claimed, 2, Some((scoped(3, 5), 20)), 10, None);
    assert_eq!(
        (
            record.id(),
            record.state(),
            record.attempts(),
            record.latest().map(|c| (c.attempt(), c.lease_expires())),
            record.available_at(),
            record.failed(),
        ),
        (
            7,
            State::Claimed,
            2,
            Some((scoped(3, 5), at(20))),
            at(10),
            None
        )
    );
    let other_id = Claimed::new(8, 1, at(20));
    assert_eq!(
        Stored::new(7, State::Claimed, 1, other_id, at(10), None),
        None
    );
    for (state, attempts, latest, available_at, failed) in [
        // Claimed without an attempt, or an attempt without a claim.
        (State::Claimed, 0, None, 0, None),
        (State::Pending, 1, None, 0, None),
        (State::Pending, 0, Some((1, 5)), 0, None),
        // More claims than identities up to the latest.
        (State::Claimed, 3, Some((2, 5)), 0, None),
        // A never-claimed record has no failure.
        (State::Pending, 0, None, 0, Some(1)),
        // Back from a fail earlier than its delay allows.
        (State::Pending, 1, Some((1, 5)), 2, Some(3)),
        // Reaped before its lease ended.
        (State::Pending, 1, Some((1, 5)), 4, None),
        // Live or confirmed with a failure on record.
        (State::Claimed, 1, Some((1, 5)), 0, Some(1)),
        (State::Confirmed, 1, Some((1, 5)), 0, Some(1)),
        // Claimed no earlier than available and before the lease ended.
        (State::Claimed, 1, Some((1, 5)), 5, None),
        (State::Confirmed, 1, Some((1, 5)), 6, None),
        (State::Dead, 1, Some((1, 5)), 5, None),
    ] {
        assert_eq!(
            stored(state, attempts, latest, available_at, failed),
            None,
            "{state:?} {attempts} {latest:?} {available_at} {failed:?}"
        );
    }
    let overflowing = Claimed::new(7, 1, Duration::MAX);
    let near_end = Duration::MAX.saturating_sub(at(1));
    assert_eq!(
        Stored::new(7, State::Dead, 1, overflowing, near_end, Some(at(2))),
        None
    );
}

#[test]
fn restored_refuses_what_this_outbox_never_reached() {
    let claimed = kept(State::Claimed, 2, Some((scoped(3, 5), 20)), 10, None);
    let outbox = Outbox::<4>::restored(max(3), 3, 5, at(12), claimed);
    let Some(outbox) = outbox else {
        panic!("a reachable state");
    };
    assert_eq!(
        (
            outbox.scope(),
            outbox.issued(),
            outbox.observed(),
            outbox.len()
        ),
        (3, 5, at(12), 1)
    );
    assert_eq!(outbox.stored(7), Some(claimed));
    assert_eq!(outbox.stored(8), None);
    // Another scope, an attempt past `issued`, and a count past the space.
    for (scope, issued) in [(4, 5), (3, 4), (3, 1 << 48)] {
        assert!(Outbox::<4>::restored(max(3), scope, issued, at(12), claimed).is_none());
    }
    // Observed before the claim, which ran no earlier than `available_at`.
    assert!(Outbox::<4>::restored(max(3), 3, 5, at(9), claimed).is_none());
    // No slot to hold it.
    assert!(Outbox::<0>::restored(max(3), 3, 5, at(12), claimed).is_none());
    // Claims against `max_attempts`.
    assert!(Outbox::<4>::restored(max(1), 3, 5, at(12), claimed).is_none());
    let retry = kept(State::Pending, 2, Some((scoped(3, 5), 20)), 30, Some(4));
    assert!(Outbox::<1>::restored(max(2), 3, 5, at(26), retry).is_none());
    assert!(Outbox::<1>::restored(max(3), 3, 5, at(26), retry).is_some());
    // Back from a fail at 26: observed no earlier than that.
    assert!(Outbox::<1>::restored(max(3), 3, 5, at(25), retry).is_none());
    let dead = kept(State::Dead, 2, Some((scoped(3, 5), 20)), 10, Some(4));
    assert!(Outbox::<1>::restored(max(3), 3, 5, at(12), dead).is_none());
    assert!(Outbox::<1>::restored(max(2), 3, 5, at(12), dead).is_some());
    let fresh = kept(State::Pending, 0, None, 10, None);
    assert!(Outbox::<1>::restored(max(1), 0, 0, at(9), fresh).is_none());
    let restored = Outbox::<1>::restored(max(1), 0, 0, at(10), fresh);
    let Some(mut restored) = restored else {
        panic!("a pending record enqueued at 10");
    };
    let mut enqueued = Outbox::<1>::new(max(1));
    assert_eq!(enqueued.enqueue(7, at(10)), Ok(Enqueue::Enqueued));
    assert_eq!(restored, enqueued);
    assert_eq!(restored.claim(at(10), at(5)), enqueued.claim(at(10), at(5)));
    assert_eq!(restored, enqueued);
}

#[test]
fn a_restored_record_answers_late_reports_like_the_original() {
    let mut outbox = Outbox::<2>::with_scope(max(3), 9);
    enqueue_all(&mut outbox, 0, &[7, 8]);
    let first = claim(&mut outbox, 1, 4);
    assert_eq!(
        outbox.fail(7, first.attempt(), at(2), at(3)),
        Ok(Failure::Failed(Disposition::Retry(at(5))))
    );
    let second = claim(&mut outbox, 2, 4);
    assert_eq!(second.id(), 8);
    let Some(record) = outbox.stored(7) else {
        panic!("7 is held");
    };
    let restored = Outbox::<1>::restored(max(3), 9, outbox.issued(), outbox.observed(), record);
    let Some(mut restored) = restored else {
        panic!("a state the outbox reached");
    };
    let late = [
        (first.attempt(), at(2), at(3)),
        (first.attempt(), at(2), at(4)),
        (second.attempt(), at(2), at(3)),
        (scoped(9, 3), at(2), at(3)),
        (scoped(8, 1), at(2), at(3)),
    ];
    for (attempt, now, retry_after) in late {
        assert_eq!(
            restored.fail(7, attempt, now, retry_after),
            outbox.fail(7, attempt, now, retry_after)
        );
        assert_eq!(restored.confirm(7, attempt), outbox.confirm(7, attempt));
    }
    let next = restored.claim(at(5), at(4));
    assert_eq!(
        next.map(|c| c.map(|c| (c.id(), c.attempt()))),
        Ok(Some((7, scoped(9, 3))))
    );
}

/// Every record the outbox holds is a state `restored` accepts, and the
/// rebuilt record answers reports and claims as the original does.
pub(super) fn restores<const N: usize>(outbox: &Outbox<N>, id: u64, seed: u64) {
    let Some(record) = outbox.stored(id) else {
        panic!("seed {seed}: {id} is held");
    };
    let rebuilt = Stored::new(
        record.id(),
        record.state(),
        record.attempts(),
        record.latest(),
        record.available_at(),
        record.failed(),
    );
    assert_eq!(rebuilt, Some(record), "seed {seed}");
    let (scope, issued, observed) = (outbox.scope(), outbox.issued(), outbox.observed());
    let Some(mut alone) = Outbox::<1>::restored(max(3), scope, issued, observed, record) else {
        panic!("seed {seed}: refused a reached state {record:?} at {observed:?}");
    };
    assert_eq!(alone.stored(id), Some(record), "seed {seed}");
    assert_eq!(alone.get(id), outbox.get(id), "seed {seed}");
    let latest = record
        .latest()
        .map_or_else(|| scoped(scope, 1), |c| c.attempt());
    let mut original = outbox.clone();
    for retry in 0..2 {
        let now = observed + at(retry);
        assert_eq!(
            alone.fail(id, latest, now, at(retry)),
            original.fail(id, latest, now, at(retry)),
            "seed {seed}"
        );
    }
    assert_eq!(
        alone.confirm(id, latest),
        original.confirm(id, latest),
        "seed {seed}"
    );
    assert_eq!(alone.get(id), original.get(id), "seed {seed}");
    assert_eq!(alone.issued(), original.issued(), "seed {seed}");
}
