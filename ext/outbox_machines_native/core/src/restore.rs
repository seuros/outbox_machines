//! Rebuilding an outbox from the record an adapter stored.
use core::num::NonZeroU32;
use core::time::Duration;

use crate::{Claimed, Lease, Outbox, Record, State, Stored};

impl Record {
    /// The record as an adapter stores it.
    pub(crate) const fn stored(&self) -> Stored {
        let latest = match self.latest {
            Some(lease) => Some(Claimed::issued(self.id, lease.attempt, lease.expires)),
            None => None,
        };
        Stored::kept(
            self.id,
            self.state,
            self.attempts,
            latest,
            self.available_at,
            self.failed,
        )
    }
}

impl<const N: usize> Outbox<N> {
    /// A table holding the one record an adapter stored, continuing the
    /// attempt sequence of `scope` after `issued` identities, with `observed`
    /// as the latest honoured instant: the rebuild of an outbox that keeps one
    /// record per row and saved [`Outbox::stored`], [`Outbox::issued`] and
    /// [`Outbox::observed`].
    ///
    /// The record is restored as the only one held, first in line and in the
    /// first slot, with the sweep cursor at its start; nothing else about the
    /// outbox it came from is kept, so a table of several records is rebuilt
    /// from one call per record, not from one call for all.
    ///
    /// `None` for a state no outbox of `max_attempts` in `scope` reaches:
    /// `issued` past `SEQUENCE_MAX` (2^48 - 1), a latest attempt this
    /// sequence never issued (another scope, or past `issued`), a pending
    /// record that has spent `max_attempts`, a claimed or confirmed one past
    /// it, a dead one short of it, an `observed` earlier than the call that
    /// last moved the record, or no slot (`N` is zero). Restoring an older
    /// record or count than the newest stored reissues identities and revives
    /// settled attempts; nothing in the values can detect that.
    #[must_use]
    pub const fn restored(
        max_attempts: NonZeroU32,
        scope: u16,
        issued: u64,
        observed: Duration,
        record: Stored,
    ) -> Option<Self> {
        let Some(mut outbox) = Self::resumed(max_attempts, scope, issued) else {
            return None;
        };
        let (attempts, max) = (record.attempts(), max_attempts.get());
        let spent = match record.state() {
            State::Pending => attempts < max,
            State::Claimed | State::Confirmed => attempts <= max,
            State::Dead => attempts == max,
        };
        let owned = match record.latest() {
            Some(claimed) => outbox.attempts.owns(claimed.attempt()),
            None => true,
        };
        if !spent || !owned || observed.as_nanos() < record.floor().as_nanos() {
            return None;
        }
        let latest = match record.latest() {
            Some(claimed) => Some(Lease {
                attempt: claimed.attempt(),
                expires: claimed.lease_expires(),
            }),
            None => None,
        };
        let Some(slot) = outbox.slots.first_mut() else {
            return None;
        };
        *slot = Some(Record {
            id: record.id(),
            seq: 0,
            state: record.state(),
            attempts,
            latest,
            available_at: record.available_at(),
            failed: record.failed(),
        });
        outbox.observed = observed;
        Some(outbox)
    }

    /// The record with this id as an adapter stores it, for
    /// [`Outbox::restored`].
    #[must_use]
    pub fn stored(&self, id: u64) -> Option<Stored> {
        self.held(id).map(Record::stored)
    }
}
