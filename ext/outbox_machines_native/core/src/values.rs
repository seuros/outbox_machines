//! The values the outbox answers with, and their validating constructors.
use core::time::Duration;

use periscope_machines::{scope_of, sequence_of};

use crate::State;

/// `true` for an attempt identity some outbox could have issued: its sequence
/// starts at 1, so sequence zero names nothing in any scope.
const fn issuable(attempt: u64) -> bool {
    sequence_of(attempt) != 0
}

/// A successful claim: the id plus the attempt identity that must accompany
/// [`Outbox::confirm`] and [`Outbox::fail`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Claimed {
    id: u64,
    attempt: u64,
    lease_expires: Duration,
}

impl Claimed {
    /// A claim value, or `None` for one no outbox answers: an `attempt` whose
    /// sequence is zero (identities start at 1), or a zero `lease_expires`
    /// (a lease is never zero, so it ends after the epoch).
    #[must_use]
    pub const fn new(id: u64, attempt: u64, lease_expires: Duration) -> Option<Self> {
        if issuable(attempt) && !lease_expires.is_zero() {
            Some(Self::issued(id, attempt, lease_expires))
        } else {
            None
        }
    }

    /// The claim the outbox just made; it upholds what [`Claimed::new`] checks.
    pub(crate) const fn issued(id: u64, attempt: u64, lease_expires: Duration) -> Self {
        Self {
            id,
            attempt,
            lease_expires,
        }
    }

    /// Claimed record id.
    #[must_use]
    pub const fn id(&self) -> u64 {
        self.id
    }

    /// Identity of this attempt: issued once for the lifetime of the outbox
    /// and never again, not even after the record is forgotten and its id
    /// enqueued anew. It carries the outbox's scope above a sequence that
    /// starts at 1, so scope zero issues 1, 2, 3. It is not the record's
    /// attempt count; that is [`Entry::attempts`].
    #[must_use]
    pub const fn attempt(&self) -> u64 {
        self.attempt
    }

    /// Instant at which the lease may be reaped by [`Outbox::expire_claims`].
    #[must_use]
    pub const fn lease_expires(&self) -> Duration {
        self.lease_expires
    }

    /// The scope of the outbox that issued this attempt, see
    /// [`Outbox::with_scope`].
    #[must_use]
    pub const fn scope(&self) -> u16 {
        scope_of(self.attempt)
    }
}

/// Where a failed or expired attempt went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    /// Back to pending, claimable at or after the given instant.
    Retry(Duration),
    /// Attempts exhausted.
    Dead,
}

/// One claim reaped by [`Outbox::expire_claims`].
///
/// An expired attempt is an *unknown* outcome: the worker may still be running
/// or may have produced the external effect. Reconcile before assuming a retry
/// is safe.
///
/// `Expired::default()` is a placeholder for pre-filling the output buffer:
/// its attempt is 0, which no outbox of any scope issues, so the outbox never
/// emits it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Expired {
    id: u64,
    attempt: u64,
    disposition: Disposition,
}

impl Expired {
    /// A reaped claim, or `None` for an `attempt` whose sequence is zero: no
    /// outbox issues it, so none reaps it. The placeholder with attempt 0 is
    /// [`Expired::default`] only.
    #[must_use]
    pub const fn new(id: u64, attempt: u64, disposition: Disposition) -> Option<Self> {
        if issuable(attempt) {
            Some(Self::reaped(id, attempt, disposition))
        } else {
            None
        }
    }

    /// A claim the outbox just reaped; its attempt was issued, so it upholds
    /// what [`Expired::new`] checks.
    pub(crate) const fn reaped(id: u64, attempt: u64, disposition: Disposition) -> Self {
        Self {
            id,
            attempt,
            disposition,
        }
    }

    /// Record id.
    #[must_use]
    pub const fn id(&self) -> u64 {
        self.id
    }

    /// Identity of the attempt whose lease ran out.
    #[must_use]
    pub const fn attempt(&self) -> u64 {
        self.attempt
    }

    /// What became of the record.
    #[must_use]
    pub const fn disposition(&self) -> Disposition {
        self.disposition
    }

    /// The scope of the outbox that issued the reaped attempt.
    #[must_use]
    pub const fn scope(&self) -> u16 {
        scope_of(self.attempt)
    }
}

impl Default for Expired {
    fn default() -> Self {
        Self::reaped(0, 0, Disposition::Dead)
    }
}

/// Where the sweep cursor stands after an [`Outbox::expire_claims`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pass {
    /// Some slot has not been examined since the last full pass: call again.
    Partial,
    /// The cursor completed a full pass over every slot, cumulatively across
    /// calls; the next call starts a new pass.
    Complete,
}

/// Progress of one bounded [`Outbox::expire_claims`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sweep {
    written: usize,
    scanned: usize,
    pass: Pass,
}

impl Sweep {
    /// A sweep report, or `None` when `written` exceeds `scanned`: every
    /// entry written came from a slot the call examined.
    #[must_use]
    pub const fn new(written: usize, scanned: usize, pass: Pass) -> Option<Self> {
        if written <= scanned {
            Some(Self::swept(written, scanned, pass))
        } else {
            None
        }
    }

    /// The report of a sweep that wrote only entries it scanned.
    pub(crate) const fn swept(written: usize, scanned: usize, pass: Pass) -> Self {
        Self {
            written,
            scanned,
            pass,
        }
    }

    /// Entries written to the caller's buffer.
    #[must_use]
    pub const fn written(&self) -> usize {
        self.written
    }

    /// Slots examined by this call.
    #[must_use]
    pub const fn scanned(&self) -> usize {
        self.scanned
    }

    /// Where the cursor stands: call again until it is [`Pass::Complete`].
    #[must_use]
    pub const fn pass(&self) -> Pass {
        self.pass
    }
}

/// A read-only view of one record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    id: u64,
    state: State,
    attempts: u32,
    attempt: Option<u64>,
    available_at: Duration,
}

impl Entry {
    /// A record view, or `None` for one no outbox holds. `attempt` is `None`
    /// exactly when `attempts` is zero, a present `attempt` has a sequence of
    /// at least 1, and only a [`State::Pending`] record may be unclaimed.
    #[must_use]
    pub const fn new(
        id: u64,
        state: State,
        attempts: u32,
        attempt: Option<u64>,
        available_at: Duration,
    ) -> Option<Self> {
        let claimed = match attempt {
            Some(attempt) if issuable(attempt) => true,
            Some(_) => return None,
            None => false,
        };
        let reachable = match state {
            State::Pending => true,
            State::Claimed | State::Confirmed | State::Dead => claimed,
        };
        if reachable && claimed == (attempts != 0) {
            Some(Self::held(id, state, attempts, attempt, available_at))
        } else {
            None
        }
    }

    /// The view of a record the outbox holds; it upholds what [`Entry::new`]
    /// checks.
    pub(crate) const fn held(
        id: u64,
        state: State,
        attempts: u32,
        attempt: Option<u64>,
        available_at: Duration,
    ) -> Self {
        Self {
            id,
            state,
            attempts,
            attempt,
            available_at,
        }
    }

    /// Record id.
    #[must_use]
    pub const fn id(&self) -> u64 {
        self.id
    }

    /// Current state.
    #[must_use]
    pub const fn state(&self) -> State {
        self.state
    }

    /// Claims issued to this record so far; it dies at `max_attempts`.
    #[must_use]
    pub const fn attempts(&self) -> u32 {
        self.attempts
    }

    /// Identity of the latest attempt, the one [`Claimed::attempt`] carried;
    /// `None` until the record is first claimed.
    #[must_use]
    pub const fn attempt(&self) -> Option<u64> {
        self.attempt
    }

    /// Earliest instant a pending record may be claimed.
    #[must_use]
    pub const fn available_at(&self) -> Duration {
        self.available_at
    }
}
