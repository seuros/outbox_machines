//! The value classes, resolved once at init and pinned. The exact-class
//! reads and clone-then-keep are `matryona_machines::ruby`'s.
use magnus::{Error, RModule, Ruby};
use matryona_machines::ruby::{Class, Registry};

/// The `Data` classes the binding builds and reads.
pub struct Classes {
    pub claimed: Class,
    pub retry: Class,
    pub failed: Class,
    pub already_failed: Class,
    pub expired: Class,
    pub sweep: Class,
    pub entry: Class,
}

static CLASSES: Registry<Classes> = Registry::new("outbox_machines");

/// Resolves the classes `lib/outbox_machines.rb` defined before it loaded the
/// extension. Called once, from init; never by name again.
pub fn resolve(ruby: &Ruby, module: RModule) -> Result<(), Error> {
    let class = |name| Class::resolve(ruby, module, name);
    let classes = Classes {
        claimed: class("Claimed")?,
        retry: class("Retry")?,
        failed: class("Failed")?,
        already_failed: class("AlreadyFailed")?,
        expired: class("Expired")?,
        sweep: class("Sweep")?,
        entry: class("Entry")?,
    };
    CLASSES.set(ruby, classes).map(drop)
}

/// The classes resolved at init.
pub fn classes(ruby: &Ruby) -> Result<&'static Classes, Error> {
    CLASSES.get(ruby)
}
