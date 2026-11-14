//! Private hooks behind the `Data` values: each one is valid or does not
//! exist. A wrong type raises `TypeError`; a value the core's constructor
//! answers `None` for raises `RangeError`.
use std::time::Duration;

use magnus::{Error, RStruct, Ruby, Symbol, TryConvert, Value, prelude::*};
use outbox_machines::{Claimed, Disposition, Entry, Expired, Pass, State, Sweep};

use crate::{
    Number,
    classes::{classes, exact, exact_array},
    integer, range, slots, wrong_type,
};

const NANOS_PER_SEC: u128 = 1_000_000_000;

/// An instant as it crosses the boundary inwards: Integer nanoseconds, exact
/// up to `Duration::MAX`, which is past `u64`.
#[derive(Clone, Copy)]
pub struct Instant(pub Duration);

impl TryConvert for Instant {
    fn try_convert(value: Value) -> Result<Self, Error> {
        integer(value, "expected an Integer (nanoseconds)")?;
        // Read natively: no Ruby method (a redefined `divmod`) runs.
        let out_of_range = || {
            let message = "an instant must be in 0..=Duration::MAX nanoseconds";
            range(&Ruby::get_with(value), message)
        };
        let nanos = u128::try_convert(value).map_err(|_| out_of_range())?;
        let secs = u64::try_from(nanos / NANOS_PER_SEC).map_err(|_| out_of_range())?;
        let below = u32::try_from(nanos % NANOS_PER_SEC).map_err(|_| out_of_range())?;
        Ok(Self(Duration::new(secs, below)))
    }
}

/// A state symbol: `TypeError` for a non-Symbol, `RangeError` for a Symbol
/// that names no state.
pub fn state(ruby: &Ruby, value: Value) -> Result<State, Error> {
    let symbol =
        Symbol::from_value(value).ok_or_else(|| wrong_type(ruby, "state must be a Symbol"))?;
    Ok(match &*symbol.name()? {
        "pending" => State::Pending,
        "claimed" => State::Claimed,
        "confirmed" => State::Confirmed,
        "dead" => State::Dead,
        _ => {
            let message = "state must be :pending, :claimed, :confirmed or :dead";
            return Err(range(ruby, message));
        }
    })
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
    let at: Instant = exact(ruby, value, classes(ruby)?.retry, message)?.aref(0)?;
    Ok(Disposition::Retry(at.0))
}

/// Any instant is a valid `Disposition::Retry`; the conversion is the check.
fn retry(at: Value) -> Result<(), Error> {
    Instant::try_convert(at).map(drop)
}

/// Any disposition is a valid `Failure`; reading it back is the check.
fn failure(ruby: &Ruby, value: Value) -> Result<(), Error> {
    disposition(ruby, value).map(drop)
}

fn claimed(ruby: &Ruby, id: Number, attempt: Number, lease_expires: Instant) -> Result<(), Error> {
    Claimed::new(id.0, attempt.0, lease_expires.0)
        .map(drop)
        .ok_or_else(|| {
            let message =
                "attempt must have a sequence of at least 1 and lease_expires be positive";
            range(ruby, message)
        })
}

fn expired(ruby: &Ruby, id: Number, attempt: Number, fate: Value) -> Result<(), Error> {
    Expired::new(id.0, attempt.0, disposition(ruby, fate)?)
        .map(drop)
        .ok_or_else(|| range(ruby, "attempt must have a sequence of at least 1"))
}

/// One element of a `Sweep`: exactly an `Expired`, its fields read natively
/// and checked again by `Expired::new`, so a forged one is refused.
fn reaped(ruby: &Ruby, row: Value) -> Result<(), Error> {
    let message = "expired must be an Array of Expired";
    let fields: RStruct = exact(ruby, row, classes(ruby)?.expired, message)?;
    expired(ruby, fields.aref(0)?, fields.aref(1)?, fields.aref(2)?)
}

/// A pass symbol: `TypeError` for a non-Symbol, `RangeError` for another.
fn pass(ruby: &Ruby, value: Value) -> Result<Pass, Error> {
    let symbol =
        Symbol::from_value(value).ok_or_else(|| wrong_type(ruby, "pass must be a Symbol"))?;
    match &*symbol.name()? {
        "partial" => Ok(Pass::Partial),
        "complete" => Ok(Pass::Complete),
        _ => Err(range(ruby, "pass must be :partial or :complete")),
    }
}

fn sweep(ruby: &Ruby, rows: Value, scanned: Number, done: Value) -> Result<(), Error> {
    // Iterated natively: an Array subclass could answer `all?` for itself.
    let rows = exact_array(ruby, rows, "expired must be an Array of Expired")?;
    for index in 0..rows.len() {
        let index = isize::try_from(index).map_err(|_| range(ruby, "too many rows"))?;
        reaped(ruby, rows.entry(index)?)?;
    }
    let done = pass(ruby, done)?;
    Sweep::new(rows.len(), slots(ruby, scanned)?, done)
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
    id: Number,
    held: Value,
    attempts: Number,
    attempt: Option<Number>,
    available_at: Instant,
) -> Result<(), Error> {
    let held = state(ruby, held)?;
    let attempts =
        u32::try_from(attempts.0).map_err(|_| range(ruby, "attempts must be in 0..2**32-1"))?;
    let attempt = attempt.map(|attempt| attempt.0);
    Entry::new(id.0, held, attempts, attempt, available_at.0)
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
