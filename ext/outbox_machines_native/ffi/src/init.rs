//! Private native entry point.
use crate::{CAPACITY, Outbox};
use magnus::{Error, Ruby, function, method, prelude::*};

#[magnus::init]
fn init(ruby: &Ruby) -> Result<(), Error> {
    let module = ruby.define_module("OutboxMachines")?;
    module.const_set("CAPACITY", CAPACITY)?;
    let class = module.define_class("Outbox", ruby.class_object())?;
    class.undef_default_alloc_func();
    class.define_singleton_method("new", function!(Outbox::new, 1))?;
    class.define_method("capacity", method!(Outbox::capacity, 0))?;
    class.define_method("max_attempts", method!(Outbox::max_attempts, 0))?;
    class.define_method("size", method!(Outbox::size, 0))?;
    class.define_method("empty?", method!(Outbox::is_empty, 0))?;
    class.define_method("enqueue", method!(Outbox::enqueue, 2))?;
    class.define_method("claim", method!(Outbox::claim, 2))?;
    class.define_method("confirm", method!(Outbox::confirm, 2))?;
    class.define_method("fail", method!(Outbox::fail, 4))?;
    class.define_method("expire_claims", method!(Outbox::expire_claims, 2))?;
    class.define_method("forget", method!(Outbox::forget, 1))?;
    class.define_method("get", method!(Outbox::get, 1))?;
    class.define_method("count", method!(Outbox::count, 1))?;
    Ok(())
}
