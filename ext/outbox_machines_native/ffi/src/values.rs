//! Private hooks behind the `Data` values: each one is valid or does not
//! exist. A wrong type raises `TypeError`; a value the core's constructor
//! answers `None` for raises `RangeError`.
use magnus::{Error, Ruby, Value, prelude::*};
use matryona_machines::ruby::{
    Choice, Symbols, exact_array, exact_fields, instant, integer, optional, symbol_or_fields,
};
use outbox_machines::{Claimed, Disposition, Entry, Expired, Pass, State, Sweep};

use crate::{classes::classes, range};

matryona_machines::symbols! {
    /// The core's record states as Symbols: `TypeError` for a non-Symbol,
    /// `RangeError` for a Symbol that names no state.
    pub States for State as "state" {
        Pending => "pending",
        Claimed => "claimed",
        Confirmed => "confirmed",
        Dead => "dead",
    }
}

matryona_machines::symbols! {
    /// The core's sweep passes as Symbols.
    pub Passes for Pass as "pass" {
        Partial => "partial",
        Complete => "complete",
    }
}

matryona_machines::symbols! {
    /// The core's dispositions as Symbols: `:dead` reads back; a retry is
    /// its `Retry`.
    pub Dispositions for Disposition as "a disposition" => names_only {
        Retry(..) => "retry",
        Dead => "dead",
    }
}

/// A `Retry` or `:dead`, read back into the core's `Disposition`. A `Retry`
/// must be exactly a `Retry`, and its instant is read and checked again.
fn disposition(ruby: &Ruby, value: Value) -> Result<Disposition, Error> {
    let message = "disposition must be a Retry or :dead";
    let retry = [classes(ruby)?.retry.get(ruby)];
    Ok(
        match symbol_or_fields(ruby, value, Dispositions, &retry, message)? {
            Choice::Symbol(disposition) => disposition,
            Choice::Struct(_, [at]) => Disposition::Retry(instant(at, "at")?),
        },
    )
}

/// Any instant is a valid `Disposition::Retry`; the conversion is the check.
fn retry(at: Value) -> Result<(), Error> {
    instant(at, "at").map(drop)
}

/// Any disposition is a valid `Failure`; reading it back is the check.
fn failure(ruby: &Ruby, value: Value) -> Result<(), Error> {
    disposition(ruby, value).map(drop)
}

fn claimed(ruby: &Ruby, id: Value, attempt: Value, lease_expires: Value) -> Result<(), Error> {
    let (id, attempt) = (integer(id, "id")?, integer(attempt, "attempt")?);
    Claimed::new(id, attempt, instant(lease_expires, "lease_expires")?)
        .map(drop)
        .ok_or_else(|| {
            let message =
                "attempt must have a sequence of at least 1 and lease_expires be positive";
            range(ruby, message)
        })
}

fn expired(ruby: &Ruby, id: Value, attempt: Value, fate: Value) -> Result<(), Error> {
    let (id, attempt) = (integer(id, "id")?, integer(attempt, "attempt")?);
    Expired::new(id, attempt, disposition(ruby, fate)?)
        .map(drop)
        .ok_or_else(|| range(ruby, "attempt must have a sequence of at least 1"))
}

/// One element of a `Sweep`: exactly an `Expired`, its fields read natively
/// and checked again by `Expired::new`, so a forged one is refused.
fn reaped(ruby: &Ruby, row: Value) -> Result<(), Error> {
    let message = "expired must be an Array of Expired";
    let [id, attempt, fate] =
        exact_fields::<3>(ruby, row, classes(ruby)?.expired.get(ruby), message)?;
    expired(ruby, id, attempt, fate)
}

fn sweep(ruby: &Ruby, rows: Value, scanned: Value, done: Value) -> Result<(), Error> {
    let scanned = integer(scanned, "scanned")?;
    // Iterated natively: an Array subclass could answer `all?` for itself.
    let rows = exact_array(ruby, rows, "expired must be an Array of Expired")?;
    for index in 0..rows.len() {
        let index = isize::try_from(index).map_err(|_| range(ruby, "too many rows"))?;
        reaped(ruby, rows.entry(index)?)?;
    }
    let done = Passes::parse(ruby, done)?;
    Sweep::new(rows.len(), scanned, done)
        .map(drop)
        .ok_or_else(|| {
            range(
                ruby,
                "a sweep reaps no more claims than the slots it scanned",
            )
        })
}

fn entry(
    ruby: &Ruby,
    id: Value,
    held: Value,
    attempts: Value,
    attempt: Value,
    available_at: Value,
) -> Result<(), Error> {
    let id = integer(id, "id")?;
    let attempts = integer(attempts, "attempts")?;
    let attempt = optional(attempt, "attempt")?;
    let available_at = instant(available_at, "available_at")?;
    Entry::new(
        id,
        States::parse(ruby, held)?,
        attempts,
        attempt,
        available_at,
    )
    .map(drop)
    .ok_or_else(|| {
        let message = "attempt is nil exactly when attempts is 0, has a sequence of at least \
                           1, and only a :pending record is unclaimed";
        range(ruby, message)
    })
}

pub fn define(module: magnus::RModule) -> Result<(), Error> {
    use magnus::function;
    module.define_singleton_method("__retry", function!(retry, 1))?;
    module.define_singleton_method("__failure", function!(failure, 1))?;
    module.define_singleton_method("__claimed", function!(claimed, 3))?;
    module.define_singleton_method("__expired", function!(expired, 3))?;
    module.define_singleton_method("__sweep", function!(sweep, 3))?;
    module.define_singleton_method("__entry", function!(entry, 5))?;
    Ok(())
}
