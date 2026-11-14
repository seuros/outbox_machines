//! Thin binding to the bounded outbox delivery policy.
mod classes;
mod init;
mod values;

use std::{cell::RefCell, time::Duration};

use magnus::{
    Error, Integer, RClass, Ruby, Symbol, TryConvert, Value, prelude::*, typed_data::Obj,
};

use crate::classes::{classes, keep};
use outbox_machines::{
    Confirmation, Disposition, Enqueue, Expired, Failure, Forgetting, Pass, Refusal, State,
};

/// Records one Ruby outbox holds.
const CAPACITY: usize = 64;
/// Most `Expired` values one `expire_claims` call answers.
const SWEEP_BUFFER: usize = 16;

fn range(ruby: &Ruby, message: &'static str) -> Error {
    Error::new(ruby.exception_range_error(), message)
}

fn wrong_type(ruby: &Ruby, message: &'static str) -> Error {
    Error::new(ruby.exception_type_error(), message)
}

/// `TypeError` with `message` unless `value` is an Integer.
fn integer(value: Value, message: &'static str) -> Result<(), Error> {
    if Integer::from_value(value).is_some() {
        Ok(())
    } else {
        Err(wrong_type(&Ruby::get_with(value), message))
    }
}

/// A count of slots as `usize`: `RangeError` past what the platform holds.
fn slots(ruby: &Ruby, count: Number) -> Result<usize, Error> {
    usize::try_from(count.0).map_err(|_| range(ruby, "a slot count must fit in usize"))
}

#[derive(Clone, Copy)]
struct Number(u64);

impl TryConvert for Number {
    fn try_convert(value: Value) -> Result<Self, Error> {
        integer(value, "expected an Integer (unsigned 64-bit)")?;
        u64::try_convert(value).map(Self)
    }
}

impl Number {
    /// Instants and spans cross the boundary as Integer nanoseconds.
    const fn nanos(self) -> Duration {
        Duration::from_nanos(self.0)
    }
}

/// A scope: an Integer in `0..=65535`.
#[derive(Clone, Copy)]
struct Scope(u16);

impl TryConvert for Scope {
    fn try_convert(value: Value) -> Result<Self, Error> {
        integer(value, "scope must be an Integer")?;
        u16::try_convert(value)
            .map(Self)
            .map_err(|_| range(&Ruby::get_with(value), "scope must be in 0..=65535"))
    }
}

/// The scope an attempt identity carries: `Claimed#scope`, `Expired#scope`.
/// Those values hold only issuable attempts, so the `RangeError` is for a
/// direct call with sequence zero.
fn scope_of(ruby: &Ruby, attempt: Number) -> Result<u16, Error> {
    Expired::new(0, attempt.0, Disposition::Dead)
        .map(|expired| expired.scope())
        .ok_or_else(|| range(ruby, "attempt must have a sequence of at least 1"))
}

/// An instant as Integer nanoseconds. Exact: `now + span` may pass `u64`.
fn instant(ruby: &Ruby, at: Duration) -> Integer {
    ruby.integer_from_u128(at.as_nanos())
}

#[magnus::wrap(class = "OutboxMachines::Outbox", free_immediately, size)]
struct Outbox {
    core: RefCell<outbox_machines::Outbox<CAPACITY>>,
}

/// An honoured answer that is a bare symbol, or the refusal's symbol.
fn answer(ruby: &Ruby, outcome: Result<&str, Refusal>) -> Value {
    match outcome {
        Ok(name) => ruby.to_symbol(name).as_value(),
        Err(e) => refusal(ruby, e),
    }
}

fn refusal(ruby: &Ruby, refusal: Refusal) -> Value {
    let name = match refusal {
        Refusal::Unknown => "unknown",
        Refusal::Stale => "stale",
        Refusal::NotTerminal => "not_terminal",
        Refusal::TimeRegression => "time_regression",
        Refusal::InvalidDuration => "invalid_duration",
        Refusal::Full => "full",
        Refusal::Exhausted => "exhausted",
        Refusal::Mismatch => "mismatch",
    };
    ruby.to_symbol(name).as_value()
}

fn state_symbol(ruby: &Ruby, state: State) -> Symbol {
    ruby.to_symbol(match state {
        State::Pending => "pending",
        State::Claimed => "claimed",
        State::Confirmed => "confirmed",
        State::Dead => "dead",
    })
}

/// `retry` is the `Retry` class, resolved at init.
fn disposition(ruby: &Ruby, retry: RClass, value: Disposition) -> Result<Value, Error> {
    Ok(match value {
        Disposition::Retry(at) => retry.funcall("new", (instant(ruby, at),))?,
        Disposition::Dead => ruby.to_symbol("dead").as_value(),
    })
}

// Refusals are answers (symbols), not exceptions. Only malformed input raises.
impl Outbox {
    fn new(ruby: &Ruby, max_attempts: Number) -> Result<Self, Error> {
        Self::with_scope(ruby, max_attempts, Scope(0))
    }

    fn with_scope(ruby: &Ruby, max_attempts: Number, scope: Scope) -> Result<Self, Error> {
        Self::resumed(ruby, max_attempts, scope, Number(0))
    }

    /// The core answers `None` for a count past the sequence; that is
    /// malformed input, so it raises like every other argument out of range.
    fn resumed(
        ruby: &Ruby,
        max_attempts: Number,
        scope: Scope,
        issued: Number,
    ) -> Result<Self, Error> {
        let max = u32::try_from(max_attempts.0)
            .ok()
            .and_then(std::num::NonZeroU32::new)
            .ok_or_else(|| range(ruby, "max_attempts must be in 1..2**32-1"))?;
        let core = outbox_machines::Outbox::resumed(max, scope.0, issued.0)
            .ok_or_else(|| range(ruby, "issued must be in 0..2**48-1"))?;
        Ok(Self {
            core: RefCell::new(core),
        })
    }

    fn issued(&self) -> u64 {
        self.core.borrow().issued()
    }

    fn scope(&self) -> u16 {
        self.core.borrow().scope()
    }

    fn capacity(&self) -> usize {
        self.core.borrow().capacity()
    }

    fn max_attempts(&self) -> u32 {
        self.core.borrow().max_attempts()
    }

    fn size(&self) -> usize {
        self.core.borrow().len()
    }

    fn is_empty(&self) -> bool {
        self.core.borrow().is_empty()
    }

    fn enqueue(ruby: &Ruby, this: Obj<Self>, id: Number, now: Number) -> Result<Value, Error> {
        this.check_frozen()?;
        let outcome = this.core.borrow_mut().enqueue(id.0, now.nanos());
        Ok(answer(
            ruby,
            outcome.map(|done| match done {
                Enqueue::Enqueued => "enqueued",
                Enqueue::Duplicate => "duplicate",
            }),
        ))
    }

    /// A `Claimed`, `nil` when nothing is due, or a refusal symbol.
    fn claim(ruby: &Ruby, this: Obj<Self>, now: Number, lease: Number) -> Result<Value, Error> {
        let class = classes(ruby)?.claimed;
        keep(ruby, this.as_value(), &this.core, |core| {
            match core.claim(now.nanos(), lease.nanos()) {
                Ok(Some(claimed)) => {
                    let expires = instant(ruby, claimed.lease_expires());
                    class.funcall("new", (claimed.id(), claimed.attempt(), expires))
                }
                Ok(None) => Ok(ruby.qnil().as_value()),
                Err(e) => Ok(refusal(ruby, e)),
            }
        })
    }

    fn confirm(ruby: &Ruby, this: Obj<Self>, id: Number, attempt: Number) -> Result<Value, Error> {
        this.check_frozen()?;
        let outcome = this.core.borrow_mut().confirm(id.0, attempt.0);
        Ok(answer(
            ruby,
            outcome.map(|done| match done {
                Confirmation::Confirmed => "confirmed",
                Confirmation::AlreadyConfirmed => "already_confirmed",
            }),
        ))
    }

    fn fail(
        ruby: &Ruby,
        this: Obj<Self>,
        id: Number,
        attempt: Number,
        now: Number,
        retry_after: Number,
    ) -> Result<Value, Error> {
        let classes = classes(ruby)?;
        keep(ruby, this.as_value(), &this.core, |core| {
            let (class, value) = match core.fail(id.0, attempt.0, now.nanos(), retry_after.nanos())
            {
                Ok(Failure::Failed(value)) => (classes.failed, value),
                Ok(Failure::AlreadyFailed(value)) => (classes.already_failed, value),
                Err(e) => return Ok(refusal(ruby, e)),
            };
            class.funcall("new", (disposition(ruby, classes.retry, value)?,))
        })
    }

    /// A `Sweep` holding up to 16 `Expired` values, or a refusal symbol.
    /// `RangeError` for a `max_scan` past `usize`, before the core runs.
    fn expire_claims(
        ruby: &Ruby,
        this: Obj<Self>,
        now: Number,
        max_scan: Number,
    ) -> Result<Value, Error> {
        let classes = classes(ruby)?;
        let scan = slots(ruby, max_scan)?;
        keep(ruby, this.as_value(), &this.core, |core| {
            // The core writes into a Rust buffer; values are built after.
            let mut out = [Expired::default(); SWEEP_BUFFER];
            let sweep = match core.expire_claims(now.nanos(), scan, &mut out) {
                Ok(sweep) => sweep,
                Err(e) => return Ok(refusal(ruby, e)),
            };
            let expired = ruby.ary_new_capa(sweep.written());
            for entry in out.iter().take(sweep.written()) {
                let fate = disposition(ruby, classes.retry, entry.disposition())?;
                let fields = (entry.id(), entry.attempt(), fate);
                expired.push(classes.expired.funcall::<_, _, Value>("new", fields)?)?;
            }
            expired.freeze();
            let pass = ruby.to_symbol(match sweep.pass() {
                Pass::Partial => "partial",
                Pass::Complete => "complete",
            });
            classes
                .sweep
                .funcall("new", (expired, sweep.scanned(), pass))
        })
    }

    fn forget(ruby: &Ruby, this: Obj<Self>, id: Number) -> Result<Value, Error> {
        this.check_frozen()?;
        let outcome = this.core.borrow_mut().forget(id.0);
        Ok(answer(
            ruby,
            outcome.map(|done| match done {
                Forgetting::Forgotten => "forgotten",
                Forgetting::NotHeld => "not_held",
            }),
        ))
    }

    /// An `Entry`, or `nil` when no record carries the id.
    fn get(ruby: &Ruby, this: Obj<Self>, id: Number) -> Result<Value, Error> {
        let class = classes(ruby)?.entry;
        let Some(entry) = this.core.borrow().get(id.0) else {
            return Ok(ruby.qnil().as_value());
        };
        let state = state_symbol(ruby, entry.state());
        let available_at = instant(ruby, entry.available_at());
        let fields = (
            entry.id(),
            state,
            entry.attempts(),
            entry.attempt(),
            available_at,
        );
        class.funcall("new", fields)
    }

    fn count(ruby: &Ruby, this: Obj<Self>, state: Value) -> Result<usize, Error> {
        let state = values::state(ruby, state)?;
        Ok(this.core.borrow().count(state))
    }
}
