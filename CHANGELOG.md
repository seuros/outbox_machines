# Changelog

## [0.3.0](https://github.com/seuros/outbox_machines/compare/outbox_machines/v0.2.0...outbox_machines/v0.3.0) (2026-11-13)


### ⚠ BREAKING CHANGES

* handles are issued through periscope-machines. Every instance can be built with a scope; a handle from another scope is refused before any lookup; all handle kinds of an instance share one sequence; resumed restores the sequence after a restart. An attempt from another outbox, or one this outbox never issued, answers Stale.

### Features

* scope attempt identities through periscope-machines, refuse attempts never issued, add Outbox::resumed ([f47285b](https://github.com/seuros/outbox_machines/commit/f47285b5304cbc21d6378b7f448badca23f32196))

## [0.2.0](https://github.com/seuros/outbox_machines/compare/outbox_machines/v0.1.2...outbox_machines/v0.2.0) (2026-11-13)


### ⚠ BREAKING CHANGES

* see CONVENTIONS: one exhaustive Refusal per crate, Result from every mutator, honoured idempotent repeats, Ruby mirrors the core. No compatibility aliases. Claimed and Expired carry a u64 attempt instead of a per-record generation; refused or repeated calls never advance the clock; the Error alias is gone; Ruby times are Integer nanoseconds and values mirror the core.

### Features

* never reissue attempt identities, honour repeated confirm, fail and forget, Full and Mismatch refusals ([12af815](https://github.com/seuros/outbox_machines/commit/12af815e86a5ac549ec5149642afe35c91ff7a75))

## [0.1.2](https://github.com/seuros/outbox_machines/compare/outbox_machines/v0.1.1...outbox_machines/v0.1.2) (2026-11-13)


### Features

* rename Error to Refusal and implement Default for Expired ([6f70b82](https://github.com/seuros/outbox_machines/commit/6f70b82ebad6b6f66ea67e520bbb54fcde79550a))

## [0.1.1](https://github.com/seuros/outbox_machines/compare/outbox_machines-v0.1.0...outbox_machines/v0.1.1) (2026-11-12)


### Features

* initial extraction ([1f6f6ac](https://github.com/seuros/outbox_machines/commit/1f6f6ac855f31b4f1da27c3709f71bc1f0d4522c))
