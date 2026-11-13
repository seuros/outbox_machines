//! Thin binding to the bounded outbox delivery policy.
mod init;

use std::{cell::RefCell, time::Duration};

use magnus::{Error, Integer, Ruby, Symbol, TryConvert, Value, prelude::*, typed_data::Obj};
use outbox_machines::{
    Confirmation, Disposition, Enqueue, Expired, Failure, Forgetting, Refusal, State,
};

const CAPACITY: usize = 64;
const SWEEP_BUFFER: usize = 16;

#[derive(Clone, Copy)]
struct Number(u64);

impl TryConvert for Number {
    fn try_convert(value: Value) -> Result<Self, Error> {
        if Integer::from_value(value).is_none() {
            return Err(Error::new(
                Ruby::get_with(value).exception_type_error(),
                "expected an Integer (unsigned 64-bit)",
            ));
        }
        u64::try_convert(value).map(Self)
    }
}

impl Number {
    /// Instants and spans cross the boundary as Integer nanoseconds.
    const fn nanos(self) -> Duration {
        Duration::from_nanos(self.0)
    }
}

/// An instant as Integer nanoseconds. Exact: `now + span` may pass `u64`.
fn instant(ruby: &Ruby, at: Duration) -> Integer {
    ruby.integer_from_u128(at.as_nanos())
}

#[magnus::wrap(class = "OutboxMachines::Outbox", free_immediately, size)]
struct Outbox {
    core: RefCell<outbox_machines::Outbox<CAPACITY>>,
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

fn data(ruby: &Ruby, name: &str) -> Result<Value, Error> {
    ruby.define_module("OutboxMachines")?.const_get(name)
}

fn state_symbol(ruby: &Ruby, state: State) -> Symbol {
    ruby.to_symbol(match state {
        State::Pending => "pending",
        State::Claimed => "claimed",
        State::Confirmed => "confirmed",
        State::Dead => "dead",
    })
}

fn disposition(ruby: &Ruby, value: Disposition) -> Result<Value, Error> {
    Ok(match value {
        Disposition::Retry(at) => data(ruby, "Retry")?.funcall("new", (instant(ruby, at),))?,
        Disposition::Dead => ruby.to_symbol("dead").as_value(),
    })
}

// Refusals are answers (symbols), not exceptions. Only malformed input raises.
impl Outbox {
    fn new(ruby: &Ruby, max_attempts: Number) -> Result<Self, Error> {
        let max = u32::try_from(max_attempts.0)
            .ok()
            .and_then(std::num::NonZeroU32::new)
            .ok_or_else(|| {
                Error::new(
                    ruby.exception_range_error(),
                    "max_attempts must be in 1..2**32-1",
                )
            })?;
        Ok(Self {
            core: RefCell::new(outbox_machines::Outbox::new(max)),
        })
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
        Ok(match outcome {
            Ok(Enqueue::Enqueued) => ruby.to_symbol("enqueued").as_value(),
            Ok(Enqueue::Duplicate) => ruby.to_symbol("duplicate").as_value(),
            Err(e) => refusal(ruby, e),
        })
    }

    /// A `Claimed`, `nil` when nothing is due, or a refusal symbol.
    fn claim(ruby: &Ruby, this: Obj<Self>, now: Number, lease: Number) -> Result<Value, Error> {
        this.check_frozen()?;
        let outcome = this.core.borrow_mut().claim(now.nanos(), lease.nanos());
        match outcome {
            Ok(Some(claimed)) => {
                let expires = instant(ruby, claimed.lease_expires);
                data(ruby, "Claimed")?.funcall("new", (claimed.id, claimed.attempt, expires))
            }
            Ok(None) => Ok(ruby.qnil().as_value()),
            Err(e) => Ok(refusal(ruby, e)),
        }
    }

    fn confirm(ruby: &Ruby, this: Obj<Self>, id: Number, attempt: Number) -> Result<Value, Error> {
        this.check_frozen()?;
        Ok(match this.core.borrow_mut().confirm(id.0, attempt.0) {
            Ok(Confirmation::Confirmed) => ruby.to_symbol("confirmed").as_value(),
            Ok(Confirmation::AlreadyConfirmed) => ruby.to_symbol("already_confirmed").as_value(),
            Err(e) => refusal(ruby, e),
        })
    }

    fn fail(
        ruby: &Ruby,
        this: Obj<Self>,
        id: Number,
        attempt: Number,
        now: Number,
        retry_after: Number,
    ) -> Result<Value, Error> {
        this.check_frozen()?;
        let outcome =
            this.core
                .borrow_mut()
                .fail(id.0, attempt.0, now.nanos(), retry_after.nanos());
        let (name, value) = match outcome {
            Ok(Failure::Failed(value)) => ("Failed", value),
            Ok(Failure::AlreadyFailed(value)) => ("AlreadyFailed", value),
            Err(e) => return Ok(refusal(ruby, e)),
        };
        data(ruby, name)?.funcall("new", (disposition(ruby, value)?,))
    }

    /// A `Sweep` holding up to 16 `Expired` values, or a refusal symbol.
    fn expire_claims(
        ruby: &Ruby,
        this: Obj<Self>,
        now: Number,
        max_scan: Number,
    ) -> Result<Value, Error> {
        this.check_frozen()?;
        let mut out = [Expired::default(); SWEEP_BUFFER];
        let scan = usize::try_from(max_scan.0).unwrap_or(usize::MAX);
        let sweep = this
            .core
            .borrow_mut()
            .expire_claims(now.nanos(), scan, &mut out);
        let sweep = match sweep {
            Ok(sweep) => sweep,
            Err(e) => return Ok(refusal(ruby, e)),
        };
        let class = data(ruby, "Expired")?;
        let expired = ruby.ary_new_capa(sweep.written);
        for entry in out.iter().take(sweep.written) {
            let fate = disposition(ruby, entry.disposition)?;
            let row: Value = class.funcall("new", (entry.id, entry.attempt, fate))?;
            expired.push(row)?;
        }
        expired.freeze();
        data(ruby, "Sweep")?.funcall("new", (expired, sweep.scanned, sweep.complete))
    }

    fn forget(ruby: &Ruby, this: Obj<Self>, id: Number) -> Result<Value, Error> {
        this.check_frozen()?;
        Ok(match this.core.borrow_mut().forget(id.0) {
            Ok(Forgetting::Forgotten) => ruby.to_symbol("forgotten").as_value(),
            Ok(Forgetting::NotHeld) => ruby.to_symbol("not_held").as_value(),
            Err(e) => refusal(ruby, e),
        })
    }

    /// An `Entry`, or `nil` when no record carries the id.
    fn get(ruby: &Ruby, this: Obj<Self>, id: Number) -> Result<Value, Error> {
        let Some(entry) = this.core.borrow().get(id.0) else {
            return Ok(ruby.qnil().as_value());
        };
        let state = state_symbol(ruby, entry.state);
        let available_at = instant(ruby, entry.available_at);
        let fields = (entry.id, state, entry.attempts, entry.attempt, available_at);
        data(ruby, "Entry")?.funcall("new", fields)
    }

    fn count(ruby: &Ruby, this: Obj<Self>, state: Symbol) -> Result<usize, Error> {
        let name = state.name()?;
        let state = match &*name {
            "pending" => State::Pending,
            "claimed" => State::Claimed,
            "confirmed" => State::Confirmed,
            "dead" => State::Dead,
            _ => {
                return Err(Error::new(
                    ruby.exception_arg_error(),
                    "state must be :pending, :claimed, :confirmed or :dead",
                ));
            }
        };
        Ok(this.core.borrow().count(state))
    }
}
