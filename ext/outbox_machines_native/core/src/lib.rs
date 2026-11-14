#![no_std]
#![doc = include_str!("../README.md")]

mod refusal;
mod values;

use core::num::NonZeroU32;
use core::time::Duration;

use periscope_machines::Issuer;

pub use refusal::Refusal;
pub use values::{Claimed, Disposition, Entry, Expired, Pass, Sweep};

/// Delivery state of one record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Waiting to be claimed, no earlier than its availability instant.
    Pending,
    /// Owned by one attempt until its lease expires.
    Claimed,
    /// The caller asserted that the external effect was persisted.
    Confirmed,
    /// Attempts are exhausted; the record will not be claimed again.
    Dead,
}

/// Honoured outcome of [`Outbox::enqueue`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Enqueue {
    /// The id is now pending.
    Enqueued,
    /// An idempotent repeat: a record with this id is still present, in any
    /// state. Nothing changed, not even the observed clock.
    Duplicate,
}

/// Honoured outcome of [`Outbox::confirm`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confirmation {
    /// The live attempt is now confirmed.
    Confirmed,
    /// An idempotent repeat: this same attempt already confirmed the record.
    /// Nothing changed.
    AlreadyConfirmed,
}

/// Honoured outcome of [`Outbox::fail`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// The live attempt is over; the record went where the disposition says.
    Failed(Disposition),
    /// An idempotent repeat: `fail` already ended this same attempt with this
    /// same `retry_after`, and no newer attempt exists. Nothing changed; the
    /// disposition is the standing one from the first call.
    AlreadyFailed(Disposition),
}

/// Honoured outcome of [`Outbox::forget`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Forgetting {
    /// The terminal record was dropped; its slot and its id are free.
    Forgotten,
    /// No record carries this id: an idempotent repeat that changed nothing.
    NotHeld,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Record {
    id: u64,
    /// Enqueue order among the records held: the held records carry exactly
    /// `0..len`, so it never outgrows the capacity.
    seq: usize,
    state: State,
    attempts: u32,
    /// Identity of the latest attempt; 0, which is never issued, until the
    /// record is first claimed.
    attempt: u64,
    available_at: Duration,
    lease_expires: Duration,
    /// The `retry_after` of the `fail` that ended the latest attempt; `None`
    /// when it is live, confirmed, or was reaped by `expire_claims`. It is
    /// what lets a repeated `fail` be told from a report on an attempt whose
    /// outcome is unknown, and from a different report on the same attempt.
    failed: Option<Duration>,
}

impl Record {
    const fn view(&self) -> Entry {
        let attempt = if self.attempt == 0 {
            None
        } else {
            Some(self.attempt)
        };
        Entry::held(
            self.id,
            self.state,
            self.attempts,
            attempt,
            self.available_at,
        )
    }
}

/// Fixed-capacity delivery-policy table holding at most `N` records.
///
/// Lookup is a linear scan: intended for small, explicit capacities.
///
/// Equality compares the whole value, sweep cursor and observed clock
/// included: a refused call leaves the outbox equal to a clone taken before.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outbox<const N: usize> {
    slots: [Option<Record>; N],
    max_attempts: NonZeroU32,
    observed: Duration,
    /// Issues the attempt identities; it also holds the outbox's scope.
    attempts: Issuer,
    cursor: usize,
    scanned_in_pass: usize,
}

impl<const N: usize> Outbox<N> {
    /// Empty table in scope zero; a record dies after `max_attempts` claims
    /// fail or expire.
    #[must_use]
    pub const fn new(max_attempts: NonZeroU32) -> Self {
        Self::with_scope(max_attempts, 0)
    }

    /// Empty table whose attempt identities carry `scope`.
    ///
    /// An attempt identity is a plain integer, so two outboxes in the same
    /// scope issue the same ones, and a report sent to the wrong outbox can
    /// settle a live attempt there when both hold the record id. Give every
    /// outbox whose reports could meet its own scope: an attempt of another
    /// scope is then always [`Refusal::Stale`]. The outbox cannot pick a
    /// scope itself; keeping them distinct is the caller's job.
    #[must_use]
    pub const fn with_scope(max_attempts: NonZeroU32, scope: u16) -> Self {
        Self {
            slots: [None; N],
            max_attempts,
            observed: Duration::ZERO,
            attempts: Issuer::new(scope),
            cursor: 0,
            scanned_in_pass: 0,
        }
    }

    /// Empty table that continues the attempt sequence of `scope` after
    /// `issued` identities: the restart of an outbox that saved
    /// [`Outbox::issued`].
    ///
    /// The next claim carries sequence `issued + 1`, so an attempt that
    /// outlived the restart is never issued again and cannot settle a new
    /// claim. `resumed(max_attempts, scope, 0)` is [`Outbox::with_scope`].
    /// Records are not restored: enqueue them again.
    ///
    /// Resuming from a count lower than what was really issued reissues
    /// identities, so save [`Outbox::issued`] before acting on a claim. An
    /// outbox that cannot save it takes a scope no earlier incarnation used
    /// instead.
    ///
    /// `None` when `issued` is beyond `SEQUENCE_MAX` (2^48 - 1), which no
    /// outbox reaches.
    #[must_use]
    pub const fn resumed(max_attempts: NonZeroU32, scope: u16, issued: u64) -> Option<Self> {
        let Some(attempts) = Issuer::resumed(scope, issued) else {
            return None;
        };
        let mut outbox = Self::with_scope(max_attempts, scope);
        outbox.attempts = attempts;
        Some(outbox)
    }

    /// The scope carried by every attempt identity this outbox issues.
    #[must_use]
    pub const fn scope(&self) -> u16 {
        self.attempts.scope()
    }

    /// How many attempt identities this outbox has issued; also the sequence
    /// number of the latest one. Save it to restart with [`Outbox::resumed`].
    #[must_use]
    pub const fn issued(&self) -> u64 {
        self.attempts.issued()
    }

    /// Compile-time record capacity.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        N
    }

    /// Claims allowed per record before it dies.
    #[must_use]
    pub const fn max_attempts(&self) -> u32 {
        self.max_attempts.get()
    }

    /// Records currently held, in any state.
    #[must_use]
    pub fn len(&self) -> usize {
        self.slots.iter().flatten().count()
    }

    /// `true` when no record is held.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.slots.iter().all(Option::is_none)
    }

    /// Records currently in `state`.
    #[must_use]
    pub fn count(&self, state: State) -> usize {
        self.slots
            .iter()
            .flatten()
            .filter(|record| record.state == state)
            .count()
    }

    /// Snapshot of the record with this id.
    #[must_use]
    pub fn get(&self, id: u64) -> Option<Entry> {
        self.held(id).map(Record::view)
    }

    fn held(&self, id: u64) -> Option<&Record> {
        self.slots.iter().flatten().find(|record| record.id == id)
    }

    fn check_time(&self, now: Duration) -> Result<(), Refusal> {
        if now < self.observed {
            Err(Refusal::TimeRegression)
        } else {
            Ok(())
        }
    }

    /// Admit a new pending record, claimable immediately.
    ///
    /// Answers [`Enqueue::Duplicate`] while a record with this id is present;
    /// that repeat changes nothing. `now` is recorded as observed only when
    /// the record is actually admitted.
    ///
    /// # Errors
    /// [`Refusal::TimeRegression`] for backwards time (checked first, also for
    /// a repeat), [`Refusal::Full`] when every slot is occupied.
    pub fn enqueue(&mut self, id: u64, now: Duration) -> Result<Enqueue, Refusal> {
        self.check_time(now)?;
        if self.held(id).is_some() {
            return Ok(Enqueue::Duplicate);
        }
        // The newest record ranks after every held one.
        let seq = self.len();
        let slot = self
            .slots
            .iter_mut()
            .find(|slot| slot.is_none())
            .ok_or(Refusal::Full)?;
        self.observed = now;
        *slot = Some(Record {
            id,
            seq,
            state: State::Pending,
            attempts: 0,
            attempt: 0,
            available_at: now,
            lease_expires: Duration::ZERO,
            failed: None,
        });
        Ok(Enqueue::Enqueued)
    }

    /// Claim the due pending record that was enqueued first.
    ///
    /// Returns `None` when nothing is due. That is an honoured answer about
    /// `now`, so `now` is recorded as observed either way: an earlier instant
    /// is refused afterwards, which keeps "nothing was due at `now`" true.
    /// The lease is a deadline for [`Outbox::expire_claims`]; it is not
    /// enforced against late confirms.
    ///
    /// The claim is stamped with an attempt identity from a sequence the
    /// outbox never rewinds: forgetting a record does not free the identities
    /// its attempts used. A scope holds `SEQUENCE_MAX` (2^48 - 1) of them.
    ///
    /// # Errors
    /// [`Refusal::TimeRegression`], [`Refusal::InvalidDuration`] for a zero
    /// lease or a lease end that overflows, or [`Refusal::Exhausted`] when a
    /// record is due but the attempt sequence is spent.
    pub fn claim(&mut self, now: Duration, lease: Duration) -> Result<Option<Claimed>, Refusal> {
        self.check_time(now)?;
        let lease_expires = now.checked_add(lease).ok_or(Refusal::InvalidDuration)?;
        if lease.is_zero() {
            return Err(Refusal::InvalidDuration);
        }
        let pick = self
            .slots
            .iter_mut()
            .flatten()
            .filter(|record| record.state == State::Pending && record.available_at <= now)
            .min_by_key(|record| record.seq);
        let Some(record) = pick else {
            self.observed = now;
            return Ok(None);
        };
        let attempt = match self.attempts.issue() {
            Ok(attempt) => attempt,
            Err(periscope_machines::Refusal::Exhausted) => return Err(Refusal::Exhausted),
        };
        self.observed = now;
        record.attempts = record.attempts.saturating_add(1);
        record.attempt = attempt;
        record.state = State::Claimed;
        record.lease_expires = lease_expires;
        record.failed = None;
        Ok(Some(Claimed::issued(record.id, attempt, lease_expires)))
    }

    /// An attempt this outbox never issued names nothing here: one of another
    /// scope, one past what was issued, or sequence zero.
    const fn check_issued(&self, attempt: u64) -> Result<(), Refusal> {
        if self.attempts.owns(attempt) {
            Ok(())
        } else {
            Err(Refusal::Stale)
        }
    }

    /// The record `attempt` was issued to, in the order every report checks:
    /// an attempt never issued here, then an id no record carries, then an
    /// attempt that is not the record's latest.
    fn reported(&mut self, id: u64, attempt: u64) -> Result<&mut Record, Refusal> {
        self.check_issued(attempt)?;
        let record = self
            .slots
            .iter_mut()
            .flatten()
            .find(|record| record.id == id)
            .ok_or(Refusal::Unknown)?;
        if record.attempt == attempt {
            Ok(record)
        } else {
            Err(Refusal::Stale)
        }
    }

    /// Caller asserts the external effect was persisted for this attempt.
    ///
    /// Accepted even after the lease deadline provided no newer attempt has
    /// superseded it: the effect happened, so the record must not be retried.
    ///
    /// Answers [`Confirmation::AlreadyConfirmed`] when this same attempt
    /// already confirmed the record. A confirmed record is never claimed
    /// again, so its latest attempt is the confirming one: that repeat is
    /// provable for as long as the record is held, and changes nothing.
    ///
    /// # Errors
    /// In this order: [`Refusal::Stale`] for an attempt this outbox never
    /// issued (another scope, or a sequence it has not reached), before any
    /// record is looked up. [`Refusal::Unknown`] when no record carries the
    /// id, which includes a confirmed record that was since forgotten.
    /// [`Refusal::Stale`] when `attempt` is neither the live attempt nor the
    /// confirming one, which includes an attempt of an earlier record that
    /// carried this id.
    pub fn confirm(&mut self, id: u64, attempt: u64) -> Result<Confirmation, Refusal> {
        let record = self.reported(id, attempt)?;
        match record.state {
            State::Claimed => {
                record.state = State::Confirmed;
                Ok(Confirmation::Confirmed)
            }
            State::Confirmed => Ok(Confirmation::AlreadyConfirmed),
            State::Pending | State::Dead => Err(Refusal::Stale),
        }
    }

    /// Caller reports a *known* failed attempt; retry after `retry_after`, or
    /// die once `max_attempts` claims are spent.
    ///
    /// Answers [`Failure::AlreadyFailed`] when `fail` already ended this same
    /// attempt with this same `retry_after` and the record has not been
    /// claimed since. That repeat changes nothing, not even the observed
    /// clock, and carries the standing disposition. `now` is the instant of
    /// the call, so it may differ; `retry_after` is what was reported, so a
    /// different one is [`Refusal::Mismatch`], also for a record that died.
    /// A repeat's arguments are still checked like any other call's.
    ///
    /// An attempt reaped by [`Outbox::expire_claims`] was not failed: a `fail`
    /// for it is [`Refusal::Stale`].
    ///
    /// # Errors
    /// In this order: [`Refusal::Stale`] for an attempt this outbox never
    /// issued (another scope, or a sequence it has not reached), before any
    /// record is looked up, [`Refusal::Unknown`] when no record carries the
    /// id, [`Refusal::Stale`] when `attempt` is neither the live attempt nor
    /// the one `fail` just ended, [`Refusal::TimeRegression`],
    /// [`Refusal::InvalidDuration`] when `now + retry_after` overflows, and
    /// [`Refusal::Mismatch`] for a repeat with another `retry_after`.
    pub fn fail(
        &mut self,
        id: u64,
        attempt: u64,
        now: Duration,
        retry_after: Duration,
    ) -> Result<Failure, Refusal> {
        let (observed, max) = (self.observed, self.max_attempts.get());
        let record = self.reported(id, attempt)?;
        // The standing disposition of an attempt `fail` already ended.
        let standing = match (record.state, record.failed) {
            (State::Claimed, _) => None,
            (State::Pending, Some(recorded)) => {
                Some((recorded, Disposition::Retry(record.available_at)))
            }
            (State::Dead, Some(recorded)) => Some((recorded, Disposition::Dead)),
            (State::Pending | State::Dead, None) | (State::Confirmed, _) => {
                return Err(Refusal::Stale);
            }
        };
        if now < observed {
            return Err(Refusal::TimeRegression);
        }
        let available_at = now
            .checked_add(retry_after)
            .ok_or(Refusal::InvalidDuration)?;
        if let Some((recorded, disposition)) = standing {
            if recorded != retry_after {
                return Err(Refusal::Mismatch);
            }
            return Ok(Failure::AlreadyFailed(disposition));
        }
        record.failed = Some(retry_after);
        let disposition = Self::settle(record, max, available_at);
        self.observed = now;
        Ok(Failure::Failed(disposition))
    }

    const fn settle(record: &mut Record, max: u32, available_at: Duration) -> Disposition {
        if record.attempts >= max {
            record.state = State::Dead;
            Disposition::Dead
        } else {
            record.state = State::Pending;
            record.available_at = available_at;
            Disposition::Retry(available_at)
        }
    }

    /// Reap claims whose lease ended at or before `now`, writing at most
    /// `out.len()` results and examining at most `max_scan` slots.
    ///
    /// Every call that is not refused is honoured and records `now` as
    /// observed, including one that reaps nothing.
    ///
    /// Resumable: a cursor persists between calls. If `out` fills, the scan
    /// stops *before* the next due claim, so nothing is lost; call again.
    /// Expired claims retry immediately (or die at `max_attempts`).
    ///
    /// The first [`Sweep::written`] entries of `out` are written; the rest are
    /// left untouched. Build the buffer as `[Expired::default(); K]`.
    ///
    /// # Errors
    /// [`Refusal::TimeRegression`].
    pub fn expire_claims(
        &mut self,
        now: Duration,
        max_scan: usize,
        out: &mut [Expired],
    ) -> Result<Sweep, Refusal> {
        self.check_time(now)?;
        self.observed = now;
        let max = self.max_attempts.get();
        let (mut written, mut scanned) = (0, 0);
        while scanned < max_scan && self.scanned_in_pass < N {
            // `cursor < N` always; an empty slot and that impossible miss
            // alike hold nothing to reap.
            if let Some(Some(record)) = self.slots.get_mut(self.cursor)
                && record.state == State::Claimed
                && record.lease_expires <= now
            {
                let Some(entry) = out.get_mut(written) else {
                    break;
                };
                let disposition = Self::settle(record, max, now);
                *entry = Expired::reaped(record.id, record.attempt, disposition);
                written += 1;
            }
            scanned += 1;
            self.scanned_in_pass += 1;
            self.cursor = (self.cursor + 1) % N;
        }
        let complete = self.scanned_in_pass >= N;
        if complete {
            self.scanned_in_pass = 0;
        }
        let pass = if complete {
            Pass::Complete
        } else {
            Pass::Partial
        };
        Ok(Sweep::swept(written, scanned, pass))
    }

    /// Drop a confirmed or dead record, freeing its slot and its id.
    ///
    /// The id may be enqueued again as a new record. The attempt identities
    /// the dropped record used are not freed: a late report carrying one is
    /// [`Refusal::Stale`] against the new record.
    ///
    /// Answers [`Forgetting::NotHeld`] when no record carries the id: the
    /// record is already gone, so that repeat is honoured and changes nothing.
    ///
    /// # Errors
    /// [`Refusal::NotTerminal`] for a pending or claimed record.
    pub fn forget(&mut self, id: u64) -> Result<Forgetting, Refusal> {
        let Some(record) = self.held(id) else {
            return Ok(Forgetting::NotHeld);
        };
        match record.state {
            State::Confirmed | State::Dead => {}
            State::Pending | State::Claimed => return Err(Refusal::NotTerminal),
        }
        // Ranks are unique: drop that one and close the gap, so the held
        // records keep ranks `0..len`.
        let gone = record.seq;
        for slot in &mut self.slots {
            match slot {
                Some(record) if record.seq == gone => *slot = None,
                Some(record) if record.seq > gone => record.seq -= 1,
                Some(_) | None => {}
            }
        }
        Ok(Forgetting::Forgotten)
    }
}

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests;
