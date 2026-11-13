# Changelog

## [0.4.0](https://github.com/seuros/outbox_machines/compare/outbox-machines-v0.3.0...outbox-machines-v0.4.0) (2026-11-13)


### ⚠ BREAKING CHANGES

* see CONVENTIONS: one exhaustive Refusal per crate, Result from every mutator, honoured idempotent repeats, Ruby mirrors the core. No compatibility aliases. Claimed and Expired carry a u64 attempt instead of a per-record generation; refused or repeated calls never advance the clock; the Error alias is gone; Ruby times are Integer nanoseconds and values mirror the core.

### Features

* never reissue attempt identities, honour repeated confirm, fail and forget, Full and Mismatch refusals ([12af815](https://github.com/seuros/outbox_machines/commit/12af815e86a5ac549ec5149642afe35c91ff7a75))

## [0.3.0](https://github.com/seuros/outbox_machines/compare/outbox-machines-v0.2.0...outbox-machines-v0.3.0) (2026-11-13)


### Features

* rename Error to Refusal and implement Default for Expired ([6f70b82](https://github.com/seuros/outbox_machines/commit/6f70b82ebad6b6f66ea67e520bbb54fcde79550a))

## [0.2.0](https://github.com/seuros/outbox_machines/compare/outbox-machines-v0.1.0...outbox-machines-v0.2.0) (2026-11-12)


### Features

* initial extraction ([1f6f6ac](https://github.com/seuros/outbox_machines/commit/1f6f6ac855f31b4f1da27c3709f71bc1f0d4522c))
