//! Thin binding to the bounded outbox delivery policy.
mod classes;
mod init;
mod values;

use std::{cell::RefCell, num::NonZeroU32};

use magnus::{Error, RClass, Ruby, Symbol, Value, prelude::*, typed_data::Obj};
use matryona_machines::ruby::{duration, integer, keep, nanoseconds, symbol};

use crate::classes::classes;
use outbox_machines::{
    Confirmation, Disposition, Enqueue, Expired, Failure, Forgetting, Refusal, State,
};

/// Records one Ruby outbox holds.
const CAPACITY: usize = 64;
/// Most `Expired` values one `expire_claims` call answers.
const SWEEP_BUFFER: usize = 16;

fn range(ruby: &Ruby, message: &'static str) -> Error {
    Error::new(ruby.exception_range_error(), message)
}

/// The scope an attempt identity carries: `Claimed#scope`, `Expired#scope`.
/// Those values hold only issuable attempts, so the `RangeError` is for a
/// direct call with sequence zero.
fn scope_of(ruby: &Ruby, attempt: Value) -> Result<u16, Error> {
    Expired::new(0, integer(attempt, "attempt")?, Disposition::Dead)
        .map(|expired| expired.scope())
        .ok_or_else(|| range(ruby, "attempt must have a sequence of at least 1"))
}

#[magnus::wrap(class = "OutboxMachines::Outbox", free_immediately, size)]
struct Outbox {
    core: RefCell<outbox_machines::Outbox<CAPACITY>>,
}

/// An honoured answer that is a bare symbol, or the refusal's symbol.
fn answer(ruby: &Ruby, outcome: Result<&str, Refusal>) -> Value {
    match outcome {
        Ok(name) => symbol(ruby, name).as_value(),
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
    symbol(ruby, name).as_value()
}

fn state_symbol(ruby: &Ruby, state: State) -> Symbol {
    symbol(ruby, values::state_name(state))
}

/// `retry` is the `Retry` class, resolved at init.
fn disposition(ruby: &Ruby, retry: RClass, value: Disposition) -> Result<Value, Error> {
    Ok(match value {
        Disposition::Retry(at) => retry.funcall("new", (nanoseconds(ruby, at),))?,
        Disposition::Dead => symbol(ruby, "dead").as_value(),
    })
}

// Refusals are answers (symbols), not exceptions. Only malformed input raises.
impl Outbox {
    fn new(ruby: &Ruby, max_attempts: Value) -> Result<Self, Error> {
        Self::build(ruby, Self::max(ruby, max_attempts)?, 0, 0)
    }

    fn with_scope(ruby: &Ruby, max_attempts: Value, scope: Value) -> Result<Self, Error> {
        let max = Self::max(ruby, max_attempts)?;
        Self::build(ruby, max, integer(scope, "scope")?, 0)
    }

    fn resumed(
        ruby: &Ruby,
        max_attempts: Value,
        scope: Value,
        issued: Value,
    ) -> Result<Self, Error> {
        let max = Self::max(ruby, max_attempts)?;
        let scope = integer(scope, "scope")?;
        Self::build(ruby, max, scope, integer(issued, "issued")?)
    }

    fn max(ruby: &Ruby, max_attempts: Value) -> Result<NonZeroU32, Error> {
        NonZeroU32::new(integer(max_attempts, "max_attempts")?)
            .ok_or_else(|| range(ruby, "max_attempts must be in 1..=4294967295"))
    }

    /// The core answers `None` for a count past the sequence; that is
    /// malformed input, so it raises like every other argument out of range.
    fn build(ruby: &Ruby, max: NonZeroU32, scope: u16, issued: u64) -> Result<Self, Error> {
        let core = outbox_machines::Outbox::resumed(max, scope, issued)
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

    fn enqueue(ruby: &Ruby, this: Obj<Self>, id: Value, now: Value) -> Result<Value, Error> {
        let (id, now) = (integer(id, "id")?, duration(now, "now")?);
        this.check_frozen()?;
        let outcome = this.core.borrow_mut().enqueue(id, now);
        Ok(answer(
            ruby,
            outcome.map(|done| match done {
                Enqueue::Enqueued => "enqueued",
                Enqueue::Duplicate => "duplicate",
            }),
        ))
    }

    /// A `Claimed`, `nil` when nothing is due, or a refusal symbol.
    fn claim(ruby: &Ruby, this: Obj<Self>, now: Value, lease: Value) -> Result<Value, Error> {
        let (now, lease) = (duration(now, "now")?, duration(lease, "lease")?);
        let class = classes(ruby)?.claimed.get(ruby);
        keep(
            ruby,
            this,
            &this.core,
            |core| core.claim(now, lease),
            |claimed| match claimed {
                Ok(Some(claimed)) => {
                    let expires = nanoseconds(ruby, claimed.lease_expires());
                    class.funcall("new", (claimed.id(), claimed.attempt(), expires))
                }
                Ok(None) => Ok(ruby.qnil().as_value()),
                Err(e) => Ok(refusal(ruby, e)),
            },
        )
    }

    fn confirm(ruby: &Ruby, this: Obj<Self>, id: Value, attempt: Value) -> Result<Value, Error> {
        let (id, attempt) = (integer(id, "id")?, integer(attempt, "attempt")?);
        this.check_frozen()?;
        let outcome = this.core.borrow_mut().confirm(id, attempt);
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
        id: Value,
        attempt: Value,
        now: Value,
        retry_after: Value,
    ) -> Result<Value, Error> {
        let (id, attempt) = (integer(id, "id")?, integer(attempt, "attempt")?);
        let (now, after) = (duration(now, "now")?, duration(retry_after, "retry_after")?);
        let classes = classes(ruby)?;
        keep(
            ruby,
            this,
            &this.core,
            |core| core.fail(id, attempt, now, after),
            |failure| {
                let (class, value) = match failure {
                    Ok(Failure::Failed(value)) => (classes.failed, value),
                    Ok(Failure::AlreadyFailed(value)) => (classes.already_failed, value),
                    Err(e) => return Ok(refusal(ruby, e)),
                };
                let retry = classes.retry.get(ruby);
                class
                    .get(ruby)
                    .funcall("new", (disposition(ruby, retry, value)?,))
            },
        )
    }

    /// A `Sweep` holding up to 16 `Expired` values, or a refusal symbol.
    /// `RangeError` for a `max_scan` past `usize`, before the core runs.
    fn expire_claims(
        ruby: &Ruby,
        this: Obj<Self>,
        now: Value,
        max_scan: Value,
    ) -> Result<Value, Error> {
        let (now, scan) = (duration(now, "now")?, integer(max_scan, "max_scan")?);
        let classes = classes(ruby)?;
        let (retry, expired_class) = (classes.retry.get(ruby), classes.expired.get(ruby));
        keep(
            ruby,
            this,
            &this.core,
            |core| {
                // The core writes into a Rust buffer; values are built after.
                let mut out = [Expired::default(); SWEEP_BUFFER];
                core.expire_claims(now, scan, &mut out)
                    .map(|sweep| (sweep, out))
            },
            |swept| {
                let (sweep, out) = match swept {
                    Ok(swept) => swept,
                    Err(e) => return Ok(refusal(ruby, e)),
                };
                let expired = ruby.ary_new_capa(sweep.written());
                for entry in out.iter().take(sweep.written()) {
                    let fate = disposition(ruby, retry, entry.disposition())?;
                    let fields = (entry.id(), entry.attempt(), fate);
                    expired.push(expired_class.funcall::<_, _, Value>("new", fields)?)?;
                }
                expired.freeze();
                let pass = symbol(ruby, values::pass_name(sweep.pass()));
                classes
                    .sweep
                    .get(ruby)
                    .funcall("new", (expired, sweep.scanned(), pass))
            },
        )
    }

    fn forget(ruby: &Ruby, this: Obj<Self>, id: Value) -> Result<Value, Error> {
        let id = integer(id, "id")?;
        this.check_frozen()?;
        let outcome = this.core.borrow_mut().forget(id);
        Ok(answer(
            ruby,
            outcome.map(|done| match done {
                Forgetting::Forgotten => "forgotten",
                Forgetting::NotHeld => "not_held",
            }),
        ))
    }

    /// An `Entry`, or `nil` when no record carries the id.
    fn get(ruby: &Ruby, this: Obj<Self>, id: Value) -> Result<Value, Error> {
        let id = integer(id, "id")?;
        let class = classes(ruby)?.entry.get(ruby);
        let Some(entry) = this.core.borrow().get(id) else {
            return Ok(ruby.qnil().as_value());
        };
        let state = state_symbol(ruby, entry.state());
        let available_at = nanoseconds(ruby, entry.available_at());
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
