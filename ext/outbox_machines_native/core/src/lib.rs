#![no_std]
#![doc = include_str!("../README.md")]

use core::num::NonZeroU32;
use core::time::Duration;

/// Delivery state of one record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
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

/// Refusal reasons. A refusal never mutates the outbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Refusal {
    /// No record carries this id.
    Unknown,
    /// The record is not claimed, or the generation belongs to another attempt.
    Stale,
    /// The record is not terminal, so it cannot be forgotten.
    NotTerminal,
    /// `now` is earlier than a previously observed instant.
    TimeRegression,
    /// A zero lease or an instant that overflows [`Duration`].
    InvalidDuration,
    /// The enqueue sequence space is exhausted.
    Exhausted,
}

/// The former name of [`Refusal`].
#[deprecated(note = "renamed to `Refusal`")]
pub type Error = Refusal;

/// Result of [`Outbox::enqueue`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Enqueue {
    /// The id is now pending.
    Enqueued,
    /// Every slot is occupied; nothing was evicted.
    Full,
    /// A record with this id is still present, in any state.
    Duplicate,
}

/// A successful claim: the id plus the generation that must accompany
/// [`Outbox::confirm`] and [`Outbox::fail`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Claimed {
    /// Claimed record id.
    pub id: u64,
    /// Attempt generation; the 1-based count of claims this record has had.
    pub generation: u32,
    /// Instant at which the lease may be reaped by [`Outbox::expire_claims`].
    pub lease_expires: Duration,
}

/// Where a failed or expired attempt went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
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
/// its generation is 0, and generations are 1-based, so the outbox never
/// emits it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Expired {
    /// Record id.
    pub id: u64,
    /// Generation of the attempt whose lease ran out.
    pub generation: u32,
    /// What became of the record.
    pub disposition: Disposition,
}

impl Default for Expired {
    fn default() -> Self {
        Self {
            id: 0,
            generation: 0,
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
    /// Claims issued so far.
    pub attempts: u32,
    /// Earliest instant a pending record may be claimed.
    pub available_at: Duration,
}

#[derive(Debug, Clone, Copy)]
struct Record {
    id: u64,
    seq: u64,
    state: State,
    attempts: u32,
    available_at: Duration,
    lease_expires: Duration,
}

impl Record {
    const fn view(&self) -> Entry {
        Entry {
            id: self.id,
            state: self.state,
            attempts: self.attempts,
            available_at: self.available_at,
        }
    }
}

/// Fixed-capacity delivery-policy table holding at most `N` records.
///
/// Lookup is a linear scan: intended for small, explicit capacities.
#[derive(Debug, Clone)]
pub struct Outbox<const N: usize> {
    slots: [Option<Record>; N],
    max_attempts: NonZeroU32,
    observed: Duration,
    next_seq: u64,
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

    fn observe(&mut self, now: Duration) -> Result<(), Refusal> {
        if now < self.observed {
            return Err(Refusal::TimeRegression);
        }
        self.observed = now;
        Ok(())
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
    /// # Errors
    /// [`Refusal::TimeRegression`] for backwards time, [`Refusal::Exhausted`] when
    /// the enqueue sequence is spent.
    pub fn enqueue(&mut self, id: u64, now: Duration) -> Result<Enqueue, Refusal> {
        self.check_time(now)?;
        if self.find(id).is_some() {
            self.observe(now)?;
            return Ok(Enqueue::Duplicate);
        }
        let Some(slot) = self.slots.iter().position(Option::is_none) else {
            self.observe(now)?;
            return Ok(Enqueue::Full);
        };
        let seq = self.next_seq;
        let next = seq.checked_add(1).ok_or(Refusal::Exhausted)?;
        self.observe(now)?;
        self.next_seq = next;
        self.slots[slot] = Some(Record {
            id,
            seq,
            state: State::Pending,
            attempts: 0,
            available_at: now,
            lease_expires: Duration::ZERO,
        });
        Ok(Enqueue::Enqueued)
    }

    /// Claim the due pending record that was enqueued first.
    ///
    /// Returns `None` when nothing is due. The lease is a deadline for
    /// [`Outbox::expire_claims`]; it is not enforced against late confirms.
    ///
    /// # Errors
    /// [`Refusal::TimeRegression`], or [`Refusal::InvalidDuration`] for a zero
    /// lease or a lease end that overflows.
    pub fn claim(&mut self, now: Duration, lease: Duration) -> Result<Option<Claimed>, Refusal> {
        self.check_time(now)?;
        let lease_expires = now.checked_add(lease).ok_or(Refusal::InvalidDuration)?;
        if lease.is_zero() {
            return Err(Refusal::InvalidDuration);
        }
        self.observe(now)?;
        let pick = self
            .slots
            .iter()
            .enumerate()
            .filter_map(|(slot, record)| record.as_ref().map(|record| (slot, record)))
            .filter(|(_, record)| record.state == State::Pending && record.available_at <= now)
            .min_by_key(|(_, record)| record.seq)
            .map(|(slot, _)| slot);
        let Some(record) = pick.and_then(|slot| self.slots[slot].as_mut()) else {
            return Ok(None);
        };
        record.attempts = record.attempts.saturating_add(1);
        record.state = State::Claimed;
        record.lease_expires = lease_expires;
        Ok(Some(Claimed {
            id: record.id,
            generation: record.attempts,
            lease_expires,
        }))
    }

    fn claimed_slot(&self, id: u64, generation: u32) -> Result<usize, Refusal> {
        let slot = self.find(id).ok_or(Refusal::Unknown)?;
        match &self.slots[slot] {
            Some(record) if record.state == State::Claimed && record.attempts == generation => {
                Ok(slot)
            }
            _ => Err(Refusal::Stale),
        }
    }

    /// Caller asserts the external effect was persisted for this attempt.
    ///
    /// Accepted even after the lease deadline provided no newer attempt has
    /// superseded it: the effect happened, so the record must not be retried.
    ///
    /// # Errors
    /// [`Refusal::Unknown`], or [`Refusal::Stale`] when the record is not claimed
    /// or `generation` is not the live attempt.
    pub fn confirm(&mut self, id: u64, generation: u32) -> Result<(), Refusal> {
        let slot = self.claimed_slot(id, generation)?;
        if let Some(record) = self.slots[slot].as_mut() {
            record.state = State::Confirmed;
        }
        Ok(())
    }

    /// Caller reports a *known* failed attempt; retry after `retry_after`, or
    /// die once `max_attempts` claims are spent.
    ///
    /// # Errors
    /// [`Refusal::Unknown`], [`Refusal::Stale`], [`Refusal::TimeRegression`], or
    /// [`Refusal::InvalidDuration`] when `now + retry_after` overflows.
    pub fn fail(
        &mut self,
        id: u64,
        generation: u32,
        now: Duration,
        retry_after: Duration,
    ) -> Result<Disposition, Refusal> {
        let slot = self.claimed_slot(id, generation)?;
        self.check_time(now)?;
        let available_at = now
            .checked_add(retry_after)
            .ok_or(Refusal::InvalidDuration)?;
        self.observe(now)?;
        let max = self.max_attempts.get();
        let Some(record) = self.slots[slot].as_mut() else {
            return Err(Refusal::Unknown);
        };
        Ok(Self::settle(record, max, available_at))
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
        self.observe(now)?;
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
                    let generation = record.attempts;
                    let disposition = Self::settle(record, max, now);
                    *entry = Expired {
                        id: record.id,
                        generation,
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
    /// # Errors
    /// [`Refusal::Unknown`], or [`Refusal::NotTerminal`] for pending or claimed.
    pub fn forget(&mut self, id: u64) -> Result<(), Refusal> {
        let slot = self.find(id).ok_or(Refusal::Unknown)?;
        match self.slots[slot].as_ref().map(|r| r.state) {
            Some(State::Confirmed | State::Dead) => {
                self.slots[slot] = None;
                Ok(())
            }
            _ => Err(Refusal::NotTerminal),
        }
    }
}

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests;
