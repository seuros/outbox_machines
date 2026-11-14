//! The one answer for every call that changed nothing.

/// Why a call was not honoured. A refusal changes nothing: no record, no
/// cursor, and not the observed clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// No record carries this id, and the attempt is one this outbox issued.
    /// Answered by [`Outbox::confirm`](crate::Outbox::confirm) and
    /// [`Outbox::fail`](crate::Outbox::fail) only; a forgotten record is unknown again.
    Unknown,
    /// `attempt` is neither the record's live attempt nor the attempt whose
    /// outcome the call repeats: it was superseded, reaped, issued to an
    /// earlier record that carried the same id, or never issued by this
    /// outbox (another scope, or a sequence it has not reached). The last is
    /// answered before any record is looked up, whatever the id.
    Stale,
    /// The record is not terminal, so it cannot be forgotten.
    NotTerminal,
    /// `now` is earlier than a previously observed instant.
    TimeRegression,
    /// A zero lease or an instant that overflows [`Duration`].
    InvalidDuration,
    /// The scope has no attempt identity left, so a due record cannot be
    /// claimed. Answered by [`Outbox::claim`](crate::Outbox::claim) only.
    Exhausted,
    /// Every slot is occupied; nothing was evicted.
    Full,
    /// A repeated [`Outbox::fail`](crate::Outbox::fail) names a `retry_after` other than the one
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
            Self::Exhausted => "the attempt sequence is spent",
            Self::Full => "every slot is occupied",
            Self::Mismatch => "retry_after is not the one recorded for this attempt",
        })
    }
}

impl core::error::Error for Refusal {}
