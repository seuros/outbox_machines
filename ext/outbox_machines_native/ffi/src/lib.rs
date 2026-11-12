//! Thin binding to the bounded outbox delivery policy.
mod init;

use std::{cell::RefCell, time::Duration};

use magnus::{
    Error, Integer, RArray, Ruby, Symbol, TryConvert, Value, prelude::*, typed_data::Obj,
};
use outbox_machines::{Disposition, Enqueue, Error as Refusal, Expired, State};

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
    const fn ms(self) -> Duration {
        Duration::from_millis(self.0)
    }
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
        _ => "exhausted",
    };
    ruby.to_symbol(name).as_value()
}

fn state_symbol(ruby: &Ruby, state: State) -> Symbol {
    ruby.to_symbol(match state {
        State::Pending => "pending",
        State::Claimed => "claimed",
        State::Confirmed => "confirmed",
        _ => "dead",
    })
}

fn disposition(ruby: &Ruby, value: Disposition) -> Result<Value, Error> {
    Ok(match value {
        Disposition::Retry(at) => {
            let at = u64::try_from(at.as_millis()).unwrap_or(u64::MAX);
            let pair = ruby.ary_new_capa(2);
            pair.push(ruby.to_symbol("retry"))?;
            pair.push(at)?;
            pair.as_value()
        }
        _ => ruby.to_symbol("dead").as_value(),
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

    fn enqueue(ruby: &Ruby, this: Obj<Self>, id: Number, now: Number) -> Result<Value, Error> {
        this.check_frozen()?;
        let outcome = this.core.borrow_mut().enqueue(id.0, now.ms());
        Ok(match outcome {
            Ok(Enqueue::Enqueued) => ruby.to_symbol("enqueued").as_value(),
            Ok(Enqueue::Duplicate) => ruby.to_symbol("duplicate").as_value(),
            Ok(_) => ruby.to_symbol("full").as_value(),
            Err(e) => refusal(ruby, e),
        })
    }

    /// `[id, generation]`, `nil` when nothing is due, or a refusal symbol.
    fn claim(ruby: &Ruby, this: Obj<Self>, now: Number, lease: Number) -> Result<Value, Error> {
        this.check_frozen()?;
        let outcome = this.core.borrow_mut().claim(now.ms(), lease.ms());
        match outcome {
            Ok(Some(claimed)) => {
                let attempt: Value = ruby.define_module("OutboxMachines")?.const_get("Attempt")?;
                attempt.funcall("new", (claimed.id, claimed.generation))
            }
            Ok(None) => Ok(ruby.qnil().as_value()),
            Err(e) => Ok(refusal(ruby, e)),
        }
    }

    fn confirm(
        ruby: &Ruby,
        this: Obj<Self>,
        id: Number,
        generation: Number,
    ) -> Result<Value, Error> {
        this.check_frozen()?;
        let Ok(generation) = u32::try_from(generation.0) else {
            return Ok(refusal(ruby, Refusal::Stale));
        };
        Ok(match this.core.borrow_mut().confirm(id.0, generation) {
            Ok(()) => ruby.to_symbol("confirmed").as_value(),
            Err(e) => refusal(ruby, e),
        })
    }

    fn fail(
        ruby: &Ruby,
        this: Obj<Self>,
        id: Number,
        generation: Number,
        now: Number,
        retry_after: Number,
    ) -> Result<Value, Error> {
        this.check_frozen()?;
        let Ok(generation) = u32::try_from(generation.0) else {
            return Ok(refusal(ruby, Refusal::Stale));
        };
        let outcome = this
            .core
            .borrow_mut()
            .fail(id.0, generation, now.ms(), retry_after.ms());
        match outcome {
            Ok(value) => disposition(ruby, value),
            Err(e) => Ok(refusal(ruby, e)),
        }
    }

    /// Up to `limit` (max 16) expirations as `[id, generation, disposition]`
    /// rows plus whether a full pass completed: `[rows, complete]`.
    fn expire_claims(
        ruby: &Ruby,
        this: Obj<Self>,
        now: Number,
        max_scan: Number,
    ) -> Result<Value, Error> {
        this.check_frozen()?;
        let blank = Expired {
            id: 0,
            generation: 0,
            disposition: Disposition::Dead,
        };
        let mut out = [blank; SWEEP_BUFFER];
        let scan = usize::try_from(max_scan.0).unwrap_or(usize::MAX);
        let sweep = this
            .core
            .borrow_mut()
            .expire_claims(now.ms(), scan, &mut out);
        let sweep = match sweep {
            Ok(sweep) => sweep,
            Err(e) => return Ok(refusal(ruby, e)),
        };
        let rows = ruby.ary_new_capa(sweep.written);
        for entry in out.iter().take(sweep.written) {
            let row = ruby.ary_new_capa(3);
            row.push(entry.id)?;
            row.push(entry.generation)?;
            row.push(disposition(ruby, entry.disposition)?)?;
            rows.push(row)?;
        }
        let result: RArray = ruby.ary_new_capa(2);
        result.push(rows)?;
        result.push(sweep.complete)?;
        Ok(result.as_value())
    }

    fn forget(ruby: &Ruby, this: Obj<Self>, id: Number) -> Result<Value, Error> {
        this.check_frozen()?;
        Ok(match this.core.borrow_mut().forget(id.0) {
            Ok(()) => ruby.to_symbol("forgotten").as_value(),
            Err(e) => refusal(ruby, e),
        })
    }

    fn state(ruby: &Ruby, this: Obj<Self>, id: Number) -> Value {
        this.core.borrow().get(id.0).map_or_else(
            || ruby.qnil().as_value(),
            |entry| state_symbol(ruby, entry.state).as_value(),
        )
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
