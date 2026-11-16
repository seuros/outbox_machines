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
    expired: usize,
    scanned: usize,
    pass: Pass,
}

impl Sweep {
    /// A sweep report, or `None` when `expired` exceeds `scanned`: every
    /// entry written came from a slot the call examined.
    #[must_use]
    pub const fn new(expired: usize, scanned: usize, pass: Pass) -> Option<Self> {
        if expired <= scanned {
            Some(Self::swept(expired, scanned, pass))
        } else {
            None
        }
    }

    /// The report of a sweep that wrote only entries it scanned.
    pub(crate) const fn swept(expired: usize, scanned: usize, pass: Pass) -> Self {
        Self {
            expired,
            scanned,
            pass,
        }
    }

    /// Expired claims written to the front of the caller's buffer.
    #[must_use]
    pub const fn expired(&self) -> usize {
        self.expired
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

/// One record as an adapter stores it: everything
/// [`Outbox::restored`](crate::Outbox::restored) needs to rebuild it, read
/// back from [`Outbox::stored`](crate::Outbox::stored).
///
/// `latest` is the [`Claimed`] of the record's latest attempt, its identity
/// and lease end; `failed` is the `retry_after` of the `fail` that ended it,
/// `None` when the attempt is live, confirmed, or was reaped by
/// [`Outbox::expire_claims`](crate::Outbox::expire_claims).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stored {
    id: u64,
    state: State,
    attempts: u32,
    latest: Option<Claimed>,
    available_at: Duration,
    failed: Option<Duration>,
}

impl Stored {
    /// A stored record, or `None` for one no outbox holds:
    ///
    /// - `latest` is present exactly when `attempts` is not zero, names this
    ///   `id`, and its sequence is at least `attempts` (every claim takes the
    ///   next identity).
    /// - A pending record never claimed has no `failed`. One that was claimed
    ///   is back from a `fail` (`available_at` at least `failed`, the fail's
    ///   instant plus its delay) or from a reap (`failed` is `None` and
    ///   `available_at`, the sweep's instant, is not before the lease end).
    /// - A claimed or confirmed record has no `failed`.
    /// - A claimed, confirmed or dead record was claimed at or after
    ///   `available_at` and before its lease end, so `available_at` is
    ///   earlier than the lease end. A dead record's `failed` delay added to
    ///   `available_at` does not overflow.
    #[must_use]
    pub const fn new(
        id: u64,
        state: State,
        attempts: u32,
        latest: Option<Claimed>,
        available_at: Duration,
        failed: Option<Duration>,
    ) -> Option<Self> {
        let reachable = match latest {
            None => attempts == 0 && failed.is_none() && matches!(state, State::Pending),
            Some(claimed) => {
                claimed.id == id
                    && attempts != 0
                    && sequence_of(claimed.attempt) >= attempts as u64
                    && Self::settled(state, available_at, claimed.lease_expires, failed)
            }
        };
        if reachable {
            Some(Self::kept(
                id,
                state,
                attempts,
                latest,
                available_at,
                failed,
            ))
        } else {
            None
        }
    }

    /// Whether a claimed record in `state` can stand at `available_at` with
    /// its lease ending at `expires` and `failed` as recorded.
    const fn settled(
        state: State,
        available_at: Duration,
        expires: Duration,
        failed: Option<Duration>,
    ) -> bool {
        let leased = available_at.as_nanos() < expires.as_nanos();
        match (state, failed) {
            (State::Pending, Some(delay)) => available_at.checked_sub(delay).is_some(),
            (State::Pending, None) => !leased,
            (State::Claimed | State::Confirmed, Some(_)) => false,
            (State::Dead, Some(delay)) => leased && available_at.checked_add(delay).is_some(),
            (State::Claimed | State::Confirmed | State::Dead, None) => leased,
        }
    }

    /// A record the outbox holds; it upholds what [`Stored::new`] checks.
    pub(crate) const fn kept(
        id: u64,
        state: State,
        attempts: u32,
        latest: Option<Claimed>,
        available_at: Duration,
        failed: Option<Duration>,
    ) -> Self {
        Self {
            id,
            state,
            attempts,
            latest,
            available_at,
            failed,
        }
    }

    /// The earliest instant an outbox holding this record has observed: the
    /// last call that set `available_at` ran then, or `failed` earlier for a
    /// record back from a `fail`.
    pub(crate) const fn floor(&self) -> Duration {
        match (self.state, self.failed) {
            (State::Pending, Some(delay)) => match self.available_at.checked_sub(delay) {
                Some(failed_at) => failed_at,
                None => Duration::ZERO,
            },
            (State::Pending | State::Claimed | State::Confirmed | State::Dead, _) => {
                self.available_at
            }
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

    /// Claims issued to this record so far.
    #[must_use]
    pub const fn attempts(&self) -> u32 {
        self.attempts
    }

    /// The latest attempt's identity and lease end; `None` until the record
    /// is first claimed.
    #[must_use]
    pub const fn latest(&self) -> Option<Claimed> {
        self.latest
    }

    /// Earliest instant a pending record may be claimed.
    #[must_use]
    pub const fn available_at(&self) -> Duration {
        self.available_at
    }

    /// The `retry_after` of the `fail` that ended the latest attempt.
    #[must_use]
    pub const fn failed(&self) -> Option<Duration> {
        self.failed
    }
}
