//! Private hooks behind the `Data` values: each one is valid or does not
//! exist. A wrong type raises `TypeError`; a value the core's constructor
//! answers `None` for raises `RangeError`.
use std::time::Duration;

use magnus::{Error, RStruct, Ruby, Symbol, Value, prelude::*};
use matryona_machines::ruby::{exact_array, exact_struct, integer, parse};
use outbox_machines::{Claimed, Disposition, Entry, Expired, Pass, State, Sweep};

use crate::{classes::classes, range};

const NANOS_PER_SEC: u128 = 1_000_000_000;

/// An instant as it crosses the boundary inwards: Integer nanoseconds, exact
/// up to `Duration::MAX`, which is past `u64`. Read natively: no Ruby method
/// (a redefined `divmod`) runs.
fn instant(value: Value, name: &str) -> Result<Duration, Error> {
    let nanos: u128 = integer(value, name)?;
    let out_of_range = || {
        let message = format!("{name} must be in 0..=Duration::MAX nanoseconds");
        Error::new(Ruby::get_with(value).exception_range_error(), message)
    };
    let secs = u64::try_from(nanos / NANOS_PER_SEC).map_err(|_| out_of_range())?;
    let below = u32::try_from(nanos % NANOS_PER_SEC).map_err(|_| out_of_range())?;
    Ok(Duration::new(secs, below))
}

const STATES: [State; 4] = [
    State::Pending,
    State::Claimed,
    State::Confirmed,
    State::Dead,
];

/// A state's symbol name.
pub const fn state_name(state: State) -> &'static str {
    match state {
        State::Pending => "pending",
        State::Claimed => "claimed",
        State::Confirmed => "confirmed",
        State::Dead => "dead",
    }
}

/// A pass's symbol name.
pub const fn pass_name(pass: Pass) -> &'static str {
    match pass {
        Pass::Partial => "partial",
        Pass::Complete => "complete",
    }
}

/// A state symbol: `TypeError` for a non-Symbol, `RangeError` for a Symbol
/// that names no state.
pub fn state(ruby: &Ruby, value: Value) -> Result<State, Error> {
    parse(ruby, value, "state", &STATES, state_name)
}

/// A `Retry` or `:dead`, read back into the core's `Disposition`. A `Retry`
/// must be exactly a `Retry`, and its instant is read and checked again.
fn disposition(ruby: &Ruby, value: Value) -> Result<Disposition, Error> {
    if let Some(symbol) = Symbol::from_value(value) {
        return if &*symbol.name()? == "dead" {
            Ok(Disposition::Dead)
        } else {
            Err(range(ruby, "a disposition Symbol must be :dead"))
        };
    }
    let message = "disposition must be a Retry or :dead";
    let retry = classes(ruby)?.retry.get(ruby);
    let at = exact_struct(ruby, value, retry, message)?.aref(0)?;
    Ok(Disposition::Retry(instant(at, "at")?))
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
    let fields: RStruct = exact_struct(ruby, row, classes(ruby)?.expired.get(ruby), message)?;
    expired(ruby, fields.aref(0)?, fields.aref(1)?, fields.aref(2)?)
}

/// A pass symbol: `TypeError` for a non-Symbol, `RangeError` for another.
fn pass(ruby: &Ruby, value: Value) -> Result<Pass, Error> {
    parse(
        ruby,
        value,
        "pass",
        &[Pass::Partial, Pass::Complete],
        pass_name,
    )
}

fn sweep(ruby: &Ruby, rows: Value, scanned: Value, done: Value) -> Result<(), Error> {
    let scanned = integer(scanned, "scanned")?;
    // Iterated natively: an Array subclass could answer `all?` for itself.
    let rows = exact_array(ruby, rows, "expired must be an Array of Expired")?;
    for index in 0..rows.len() {
        let index = isize::try_from(index).map_err(|_| range(ruby, "too many rows"))?;
        reaped(ruby, rows.entry(index)?)?;
    }
    let done = pass(ruby, done)?;
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
    let attempt = if attempt.is_nil() {
        None
    } else {
        Some(integer(attempt, "attempt")?)
    };
    let available_at = instant(available_at, "available_at")?;
    Entry::new(id, state(ruby, held)?, attempts, attempt, available_at)
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
