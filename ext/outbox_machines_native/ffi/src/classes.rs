//! The value classes, resolved once at init, the exact-class reads every
//! value from Ruby goes through, and the clone-then-keep every mutating call
//! whose answer is a Ruby object runs in.
use std::{cell::RefCell, sync::OnceLock};

use magnus::{Error, RArray, RClass, RModule, RStruct, Ruby, Value, prelude::*, value::Opaque};

use crate::wrong_type;

/// The `Data` classes the binding builds and reads.
#[derive(Clone, Copy)]
pub struct Classes {
    pub claimed: RClass,
    pub retry: RClass,
    pub failed: RClass,
    pub already_failed: RClass,
    pub expired: RClass,
    pub sweep: RClass,
    pub entry: RClass,
}

/// `Classes` as a static may hold them.
struct Held([Opaque<RClass>; 7]);

static HELD: OnceLock<Held> = OnceLock::new();

/// Resolves the classes `lib/outbox_machines.rb` defined before it loaded the
/// extension. Called once, from init; never by name again.
pub fn resolve(ruby: &Ruby, module: RModule) -> Result<(), Error> {
    let names = [
        "Claimed",
        "Retry",
        "Failed",
        "AlreadyFailed",
        "Expired",
        "Sweep",
        "Entry",
    ];
    let mut held = [Opaque::from(ruby.class_object()); 7];
    for (slot, name) in held.iter_mut().zip(names) {
        let class: RClass = module.const_get(name)?;
        ruby.gc_register_mark_object(class);
        *slot = Opaque::from(class);
    }
    HELD.set(Held(held)).map_err(|_| {
        Error::new(
            ruby.exception_runtime_error(),
            "outbox_machines_native loaded twice",
        )
    })
}

/// The classes resolved at init.
pub fn classes(ruby: &Ruby) -> Result<Classes, Error> {
    let Some(Held(held)) = HELD.get() else {
        return Err(Error::new(
            ruby.exception_runtime_error(),
            "outbox_machines_native is not initialised",
        ));
    };
    let [
        claimed,
        retry,
        failed,
        already_failed,
        expired,
        sweep,
        entry,
    ] = held.map(|class| ruby.get_inner(class));
    Ok(Classes {
        claimed,
        retry,
        failed,
        already_failed,
        expired,
        sweep,
        entry,
    })
}

/// Whether `value`'s class is exactly `class`: `instance_of?`, never `is_a?`,
/// and asked natively, so a redefined method cannot answer for it.
fn is_instance_of(value: Value, class: RClass) -> bool {
    let own = value.class();
    own.is_inherited(class) && class.is_inherited(own)
}

/// The fields of a `Data` value of exactly `class`; `TypeError` otherwise,
/// a subclass included. Read with [`RStruct::aref`], which runs no Ruby.
pub fn exact(
    ruby: &Ruby,
    value: Value,
    class: RClass,
    message: &'static str,
) -> Result<RStruct, Error> {
    RStruct::from_value(value)
        .filter(|_| is_instance_of(value, class))
        .ok_or_else(|| wrong_type(ruby, message))
}

/// An `Array` that is exactly an `Array`, no subclass: its elements are then
/// read natively, never through a method it could redefine.
pub fn exact_array(ruby: &Ruby, value: Value, message: &'static str) -> Result<RArray, Error> {
    RArray::from_value(value)
        .filter(|_| is_instance_of(value, ruby.class_array()))
        .ok_or_else(|| wrong_type(ruby, message))
}

/// Runs `act` on a copy of the core in `cell` and keeps the copy only once
/// `act`, which decides and then builds the Ruby answer, has returned. A raise
/// while building leaves the core as it was. Then, right before the store,
/// the receiver is checked again: frozen meanwhile is `FrozenError`, changed
/// meanwhile (a redefined `new` calling back in) is `RuntimeError`, and in
/// both cases the copy is dropped. No borrow is held while `act` runs.
pub fn keep<C: Clone + PartialEq, R>(
    ruby: &Ruby,
    receiver: Value,
    cell: &RefCell<C>,
    act: impl FnOnce(&mut C) -> Result<R, Error>,
) -> Result<R, Error> {
    receiver.check_frozen()?;
    let before = cell.borrow().clone();
    let mut after = before.clone();
    let answer = act(&mut after)?;
    receiver.check_frozen()?;
    let mut held = cell.borrow_mut();
    if *held != before {
        return Err(Error::new(
            ruby.exception_runtime_error(),
            "the outbox changed while the answer was built; the call was not applied",
        ));
    }
    *held = after;
    Ok(answer)
}
