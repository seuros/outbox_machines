# Changelog

## [0.6.1](https://github.com/seuros/outbox_machines/compare/outbox_machines/v0.6.0...outbox_machines/v0.6.1) (2026-11-14)


### Bug Fixes

* **deps:** require matryona-machines &gt;= 0.3 ([8f99e68](https://github.com/seuros/outbox_machines/commit/8f99e681d265f29452b81a86ec67adcf4b309931))

## [0.6.0](https://github.com/seuros/outbox_machines/compare/outbox_machines/v0.5.0...outbox_machines/v0.6.0) (2026-11-14)


### ⚠ BREAKING CHANGES

* adopt matryona-machines 0.2 and check frozen before converting arguments

### Features

* adopt matryona-machines 0.2 and check frozen before converting arguments ([06553b9](https://github.com/seuros/outbox_machines/commit/06553b93b8b47b79289cb85c1da24b44248146a0))

## [0.5.0](https://github.com/seuros/outbox_machines/compare/outbox_machines/v0.4.0...outbox_machines/v0.5.0) (2026-11-14)


### ⚠ BREAKING CHANGES

* build the binding on matryona-machines; accept a genuine value whose singleton class was opened; name every argument in conversion errors

### Features

* build the binding on matryona-machines; accept a genuine value whose singleton class was opened; name every argument in conversion errors ([79e565c](https://github.com/seuros/outbox_machines/commit/79e565c2859fd02c7ca7fc99057b0cca410aa988))

## [0.4.0](https://github.com/seuros/outbox_machines/compare/outbox_machines/v0.3.0...outbox_machines/v0.4.0) (2026-11-14)


### ⚠ BREAKING CHANGES

* private value fields, drop the enqueue sequence, Pass replaces the sweep flag, sealed exact-class values, clone-then-keep claim/fail/expire

### Features

* private value fields, drop the enqueue sequence, Pass replaces the sweep flag, sealed exact-class values, clone-then-keep claim/fail/expire ([e6c66dc](https://github.com/seuros/outbox_machines/commit/e6c66dcb726d2a211785504a9fff6ad1daa19611))

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
