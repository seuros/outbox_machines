#![no_std]
#![doc = include_str!("../README.md")]

use core::num::NonZeroU32;
use core::time::Duration;

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

/// Why a call was not honoured. A refusal changes nothing: no record, no
/// cursor, and not the observed clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// No record carries this id. Answered by [`Outbox::confirm`] and
    /// [`Outbox::fail`] only; a forgotten record is unknown again.
    Unknown,
    /// `attempt` is neither the record's live attempt nor the attempt whose
    /// outcome the call repeats: it was superseded, reaped, never issued, or
    /// issued to an earlier record that carried the same id.
    Stale,
    /// The record is not terminal, so it cannot be forgotten.
    NotTerminal,
    /// `now` is earlier than a previously observed instant.
    TimeRegression,
    /// A zero lease or an instant that overflows [`Duration`].
    InvalidDuration,
    /// The enqueue sequence or the attempt sequence is spent: the outbox can
    /// admit no further record, or issue no further attempt.
    Exhausted,
    /// Every slot is occupied; nothing was evicted.
    Full,
    /// A repeated [`Outbox::fail`] names a `retry_after` other than the one
    /// recorded for that attempt, so it is not the same report.
    Mismatch,
}

impl core::fmt::Display for Refusal {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::Unknown => "no record carries this id",
            Self::Stale => "the attempt is not the live one",
            Self::NotTerminal => "the record is not confirmed or dead",
            Self::TimeRegression => "now is earlier than an observed instant",
            Self::InvalidDuration => "zero lease or overflowing instant",
            Self::Exhausted => "the enqueue or attempt sequence is spent",
            Self::Full => "every slot is occupied",
            Self::Mismatch => "retry_after is not the one recorded for this attempt",
        })
    }
}

impl core::error::Error for Refusal {}

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

/// A successful claim: the id plus the attempt identity that must accompany
/// [`Outbox::confirm`] and [`Outbox::fail`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Claimed {
    /// Claimed record id.
    pub id: u64,
    /// Identity of this attempt: issued once for the lifetime of the outbox,
    /// starting at 1, and never again, not even after the record is forgotten
    /// and its id enqueued anew. It is not the record's attempt count; that is
    /// [`Entry::attempts`].
    pub attempt: u64,
    /// Instant at which the lease may be reaped by [`Outbox::expire_claims`].
    pub lease_expires: Duration,
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
/// its attempt is 0, and attempt identities start at 1, so the outbox never
/// emits it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Expired {
    /// Record id.
    pub id: u64,
    /// Identity of the attempt whose lease ran out.
    pub attempt: u64,
    /// What became of the record.
    pub disposition: Disposition,
}

impl Default for Expired {
    fn default() -> Self {
        Self {
            id: 0,
            attempt: 0,
            disposition: Disposition::Dead,
        }
    }
}

/// Progress of one bounded [`Outbox::expire_claims`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sweep {
    /// Entries written to the caller's buffer.
    pub written: usize,
    /// Slots examined by this call.
    pub scanned: usize,
    /// `true` once the cursor has completed a full pass over every slot,
    /// cumulatively across calls. Call again until it is `true`.
    pub complete: bool,
}

/// A read-only view of one record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    /// Record id.
    pub id: u64,
    /// Current state.
    pub state: State,
    /// Claims issued to this record so far; it dies at `max_attempts`.
    pub attempts: u32,
    /// Identity of the latest attempt, the one [`Claimed::attempt`] carried;
    /// `None` until the record is first claimed.
    pub attempt: Option<u64>,
    /// Earliest instant a pending record may be claimed.
    pub available_at: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Record {
    id: u64,
    seq: u64,
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
    const fn disposition(&self) -> Disposition {
        match self.state {
            State::Dead => Disposition::Dead,
            State::Pending | State::Claimed | State::Confirmed => {
                Disposition::Retry(self.available_at)
            }
        }
    }

    const fn view(&self) -> Entry {
        Entry {
            id: self.id,
            state: self.state,
            attempts: self.attempts,
            attempt: if self.attempt == 0 {
                None
            } else {
                Some(self.attempt)
            },
            available_at: self.available_at,
        }
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
    next_seq: u64,
    next_attempt: u64,
    cursor: usize,
    scanned_in_pass: usize,
}

impl<const N: usize> Outbox<N> {
    /// Empty table; a record dies after `max_attempts` claims fail or expire.
    #[must_use]
    pub const fn new(max_attempts: NonZeroU32) -> Self {
        Self {
            slots: [None; N],
            max_attempts,
            observed: Duration::ZERO,
            next_seq: 0,
            next_attempt: 1,
            cursor: 0,
            scanned_in_pass: 0,
        }
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
        self.find(id)
            .and_then(|slot| self.slots[slot].as_ref())
            .map(Record::view)
    }

    fn find(&self, id: u64) -> Option<usize> {
        self.slots
            .iter()
            .position(|slot| slot.as_ref().is_some_and(|record| record.id == id))
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
    /// a repeat), [`Refusal::Full`] when every slot is occupied,
    /// [`Refusal::Exhausted`] when the enqueue sequence is spent.
    pub fn enqueue(&mut self, id: u64, now: Duration) -> Result<Enqueue, Refusal> {
        self.check_time(now)?;
        if self.find(id).is_some() {
            return Ok(Enqueue::Duplicate);
        }
        let slot = self
            .slots
            .iter()
            .position(Option::is_none)
            .ok_or(Refusal::Full)?;
        let seq = self.next_seq;
        let next = seq.checked_add(1).ok_or(Refusal::Exhausted)?;
        self.observed = now;
        self.next_seq = next;
        self.slots[slot] = Some(Record {
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
    /// its attempts used.
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
            .iter()
            .enumerate()
            .filter_map(|(slot, record)| record.as_ref().map(|record| (slot, record)))
            .filter(|(_, record)| record.state == State::Pending && record.available_at <= now)
            .min_by_key(|(_, record)| record.seq)
            .map(|(slot, _)| slot);
        let Some(record) = pick.and_then(|slot| self.slots[slot].as_mut()) else {
            self.observed = now;
            return Ok(None);
        };
        let attempt = self.next_attempt;
        let next = attempt.checked_add(1).ok_or(Refusal::Exhausted)?;
        self.observed = now;
        self.next_attempt = next;
        record.attempts = record.attempts.saturating_add(1);
        record.attempt = attempt;
        record.state = State::Claimed;
        record.lease_expires = lease_expires;
        record.failed = None;
        Ok(Some(Claimed {
            id: record.id,
            attempt,
            lease_expires,
        }))
    }

    fn record(&mut self, id: u64) -> Result<&mut Record, Refusal> {
        self.slots
            .iter_mut()
            .flatten()
            .find(|record| record.id == id)
            .ok_or(Refusal::Unknown)
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
    /// [`Refusal::Unknown`] when no record carries the id, which includes a
    /// confirmed record that was since forgotten. [`Refusal::Stale`] when
    /// `attempt` is neither the live attempt nor the confirming one, which
    /// includes an attempt of an earlier record that carried this id.
    pub fn confirm(&mut self, id: u64, attempt: u64) -> Result<Confirmation, Refusal> {
        let record = self.record(id)?;
        if record.attempt != attempt {
            return Err(Refusal::Stale);
        }
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
    /// In this order: [`Refusal::Unknown`] when no record carries the id,
    /// [`Refusal::Stale`] when `attempt` is neither the live attempt nor the
    /// one `fail` just ended, [`Refusal::TimeRegression`],
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
        let record = self.record(id)?;
        if record.attempt != attempt {
            return Err(Refusal::Stale);
        }
        let recorded = match record.state {
            State::Claimed => None,
            State::Pending | State::Dead => Some(record.failed.ok_or(Refusal::Stale)?),
            State::Confirmed => return Err(Refusal::Stale),
        };
        if now < observed {
            return Err(Refusal::TimeRegression);
        }
        let available_at = now
            .checked_add(retry_after)
            .ok_or(Refusal::InvalidDuration)?;
        if let Some(recorded) = recorded {
            if recorded != retry_after {
                return Err(Refusal::Mismatch);
            }
            return Ok(Failure::AlreadyFailed(record.disposition()));
        }
        record.failed = Some(retry_after);
        let disposition = Self::settle(record, max, available_at);
        self.observed = now;
        Ok(Failure::Failed(disposition))
    }

    const fn settle(record: &mut Record, max: u32, available_at: Duration) -> Disposition {
        if record.attempts >= max {
            record.state = State::Dead;
        } else {
            record.state = State::Pending;
            record.available_at = available_at;
        }
        record.disposition()
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
            let due = self.slots[self.cursor]
                .as_ref()
                .is_some_and(|r| r.state == State::Claimed && r.lease_expires <= now);
            if due {
                let Some(entry) = out.get_mut(written) else {
                    break;
                };
                if let Some(record) = self.slots[self.cursor].as_mut() {
                    let disposition = Self::settle(record, max, now);
                    *entry = Expired {
                        id: record.id,
                        attempt: record.attempt,
                        disposition,
                    };
                    written += 1;
                }
            }
            scanned += 1;
            self.scanned_in_pass += 1;
            self.cursor = (self.cursor + 1) % N;
        }
        let complete = self.scanned_in_pass >= N;
        if complete {
            self.scanned_in_pass = 0;
        }
        Ok(Sweep {
            written,
            scanned,
            complete,
        })
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
        for slot in &mut self.slots {
            if let Some(record) = slot
                && record.id == id
            {
                return match record.state {
                    State::Confirmed | State::Dead => {
                        *slot = None;
                        Ok(Forgetting::Forgotten)
                    }
                    State::Pending | State::Claimed => Err(Refusal::NotTerminal),
                };
            }
        }
        Ok(Forgetting::NotHeld)
    }
}

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests;
