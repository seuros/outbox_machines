# outbox-machines

> **AMS Helland**, NCC-4221, *Helland*-class dispatch courier.

Engine-room flight manual: track owed dispatches through identity-checked attempts.

Initial, unstable. Bounded delivery-policy metadata for an outbox: records move
through `pending`, `claimed`, `confirmed` and `dead`, and every attempt carries
an identity the outbox issues once and never again, so a stale worker cannot
touch a newer attempt, nor a newer record that reuses the id.

This is **delivery policy only**. It is not a transactional outbox or inbox, not
a relay, and not a retry engine: it never sleeps, schedules, or performs I/O.
Atomic insertion with business writes, durable claims, crash recovery and
payload storage are adapter responsibilities. It composes with saga, dedup,
lease and chrono through explicit caller-side contracts only; this crate has no
dependency on any sibling.

Allocation-free `no_std`. Capacity `N` is a const generic; a full table refuses
with `Refusal::Full` and never evicts. Time is explicit and must not go
backwards: every instant (`now`, `Claimed::lease_expires`, `Disposition::Retry`,
`Entry::available_at`) and every span (`lease`, `retry_after`) is a
`core::time::Duration` since an epoch the caller picks. The Ruby gem passes
and answers the same values as Integer nanoseconds, and mirrors `Claimed`,
`Expired`, `Sweep` and `Entry` as `Data` values of the same names, and
`Failure`'s two variants as `Failed` and `AlreadyFailed` (its `Sweep` carries
the expired entries where this one counts `written`). Lookup is a linear scan, so keep `N`
small. Ids are `u64`; payloads live outside.

- `enqueue(id, now) -> Result<Enqueue, Refusal>`: `Enqueued`, or `Duplicate`
  for an id that is already present (an idempotent repeat: honoured, nothing
  changes). An id stays taken, even when confirmed or dead, until
  `forget(id)`. Refuses with `TimeRegression`, `Full` or `Exhausted`.
- `claim(now, lease) -> Result<Option<Claimed>, Refusal>`: the due pending
  record enqueued first, or `None` when nothing is due. `Claimed::attempt` is
  the identity of the attempt, drawn from one sequence for the whole outbox,
  starting at 1 and carried under the outbox's scope. Refuses with `TimeRegression`, `InvalidDuration`, or
  `Exhausted` when a record is due but that sequence is spent.
- `confirm(id, attempt) -> Result<Confirmation, Refusal>`: the caller
  asserts the external effect was persisted. The outbox cannot verify it.
  Accepted even after the lease deadline, as long as no newer attempt exists.
  `Confirmed`, or `AlreadyConfirmed` when this same attempt already
  confirmed the record. Refuses with `Stale` for an attempt this outbox never
  issued (another scope, or a sequence it has not reached; before any
  lookup), then `Unknown` or `Stale`.
- `fail(id, attempt, now, retry_after) -> Result<Failure, Refusal>`: a
  *known* failure. `Failed(Disposition)`: back to pending at
  `now + retry_after`, or `Dead` once `max_attempts` claims are spent.
  `AlreadyFailed(Disposition)` when `fail` already ended this same attempt
  with this same `retry_after` and no claim followed; it carries the standing
  disposition. Refuses, in this order, with `Stale` for an attempt this outbox
  never issued, `Unknown`, `Stale`,
  `TimeRegression`, `InvalidDuration`, or `Mismatch` for a repeat whose
  `retry_after` is not the one recorded.
- `expire_claims(now, max_scan, out)`: reaps claims whose lease ended, into a
  caller buffer, with a persistent cursor. Bounded by `max_scan` and
  `out.len()`; call again until `Sweep::complete`. Writes the first
  `Sweep::written` entries of `out` and leaves the rest untouched. Build the
  buffer as `[Expired::default(); K]`: the default is a placeholder with
  attempt 0, which the outbox never emits. Refuses with `TimeRegression`.
- `forget(id) -> Result<Forgetting, Refusal>`: drops a confirmed or dead
  record. `Forgotten`, or `NotHeld` when no record carries the id. Refuses
  with `NotTerminal`.
- `get(id) -> Option<Entry>`: the record's state, `attempts` (the claims it
  has had, which is what `max_attempts` counts) and `attempt` (the identity
  of the latest one, `None` before the first claim).

## Attempt identities

An attempt identity is issued once for the lifetime of the outbox. It is not
the record's attempt count, which restarts when an id is forgotten and
enqueued again; the identity does not. A worker that claimed id 7, went quiet,
and reports back after 7 was confirmed, forgotten, enqueued and claimed anew
presents an identity the new record never had: `Stale`, nothing changed.

## Scopes

An attempt identity means something only to the outbox that issued it. It is
a `u64` in the [`periscope-machines`](https://crates.io/crates/periscope-machines)
layout: the outbox's scope in the top 16 bits, a sequence counted from 1 in
the low 48. `Outbox::new(max_attempts)` is scope zero and issues 1, 2, 3;
`Outbox::with_scope(max_attempts, scope)` issues the same sequence under
`scope`, and `Outbox::scope()`, `Claimed::scope()` and `Expired::scope()` read
it back.

Two outboxes that hold the same record id issue the same sequence numbers, so
without scopes a report sent to the wrong outbox settles the wrong attempt.
Give each outbox its own scope: `confirm` and `fail` then answer `Stale` to an
attempt of another scope before they look at any record, and nothing changes.
The outbox reads no clock and no random source, so it cannot pick a scope;
two outboxes left in the same scope still alias.

The check is `periscope_machines::Issuer::owns`, so it refuses more than
another scope: an attempt of this scope whose sequence the outbox has not
reached, or sequence zero, was never issued here and is `Stale` too, whatever
the id.

A fresh outbox starts its sequence again, so an attempt that outlived a
restart could name a new claim. Either save `Outbox::issued()` and restart
with `Outbox::resumed(max_attempts, scope, issued)`, which continues at
`issued + 1` (`None` when `issued` is past 2^48 - 1), or give each incarnation
a scope no earlier one used. Save the count before acting on a claim:
resuming from too low a count reissues identities.

A scope holds 2^48 - 1 identities. When they are spent `claim` refuses with
`Exhausted` rather than wrap, and only when a record is due.

## Repeats and refusals

`Duplicate`, `AlreadyConfirmed`, `AlreadyFailed` and `NotHeld` are idempotent
repeats: the effect asked for already holds, so the call is honoured and
changes nothing, the observed clock included. The outbox honours only what it
can prove from the record it still holds:

- A confirmed record is never claimed again, so its latest attempt *is* the
  confirming one. `confirm` with that attempt repeats; any other attempt is
  `Stale`, an attempt of an earlier record with the same id included. The
  attempt a record holds was issued by this outbox, and an attempt of another
  scope is refused before the record is read, so a repeat is only ever
  honoured for an identity of this scope.
- A record remembers whether its latest attempt was ended by `fail`, and with
  which `retry_after`. `fail` with that attempt and that `retry_after` repeats
  until the record is claimed again. `now` is the instant of the call, not
  part of the report, so it may differ; a different `retry_after` is a
  different report and is refused with `Mismatch`, also when the record died
  and the delay shaped nothing. An attempt reaped by `expire_claims` has an
  unknown outcome, not a failed one: `fail` for it is `Stale`, as is `fail`
  for a confirmed attempt and `confirm` for a failed one.
- `claim` and `expire_claims` carry no identity a repeat could be matched on.
  A second identical call is a new call: it claims the next due record, or
  reaps nothing.

`Refusal::Unknown` is answered by `confirm` and `fail` only, when no record
carries the id and the attempt is one this outbox issued. That includes a record that was confirmed or dead and then
forgotten: nothing about it is kept, so a late repeat cannot be proven and is
`Unknown`. `enqueue`, `claim`, `expire_claims` and `forget` never answer it.

`Refusal::Stale` means the id is held but `attempt` is neither its live
attempt nor the attempt whose outcome the call repeats, or that this outbox
never issued `attempt` (another scope, or a sequence it has not reached),
whatever the id. The variants are
`Unknown`, `Stale`, `NotTerminal`, `TimeRegression`, `InvalidDuration`,
`Exhausted`, `Full` and `Mismatch`. `Refusal` is the only refusal type, no public enum is `#[non_exhaustive]`
(a new variant is a breaking change every caller matches on), and a refused call
changes nothing: no record, no sweep cursor, and not the observed clock.
`Outbox` is `PartialEq`, so `outbox == clone_taken_before` proves it.

The observed clock moves only when a call is honoured and takes effect. A
`Duplicate` enqueue and an `AlreadyFailed` fail leave it alone. A `claim` that answers `None` and an
`expire_claims` that reaps nothing were honoured answers about `now`, so they
do record it.

## Ambiguous effects

An expired lease is an unknown outcome, not a failure: the worker may still run
or may have already produced the effect. Expired claims are reported separately
so the adapter can reconcile (fence, query the destination, or use an
idempotency key) before trusting a retry. Use `fail` only for failures you know
did not take effect.

```rust
use core::num::NonZeroU32;
use core::time::Duration;
use outbox_machines::{Confirmation, Disposition, Enqueue, Failure, Forgetting, Outbox, Refusal};

let mut outbox = Outbox::<8>::new(NonZeroU32::new(3).unwrap());
assert_eq!(outbox.enqueue(42, Duration::ZERO), Ok(Enqueue::Enqueued));
let claimed = outbox
    .claim(Duration::ZERO, Duration::from_secs(30))
    .unwrap()
    .unwrap();
assert_eq!((claimed.id, claimed.attempt), (42, 1));
let retry_at_5 = Disposition::Retry(Duration::from_secs(5));
assert_eq!(
    outbox.fail(42, 1, Duration::ZERO, Duration::from_secs(5)),
    Ok(Failure::Failed(retry_at_5))
);
assert_eq!(
    outbox.fail(42, 1, Duration::ZERO, Duration::from_secs(5)),
    Ok(Failure::AlreadyFailed(retry_at_5))
);
assert_eq!(
    outbox.fail(42, 1, Duration::ZERO, Duration::from_secs(9)),
    Err(Refusal::Mismatch)
);
let retry = outbox.claim(Duration::from_secs(5), Duration::from_secs(30)).unwrap().unwrap();
assert_eq!(retry.attempt, 2);
assert_eq!(outbox.confirm(42, 1), Err(Refusal::Stale));
assert_eq!(outbox.confirm(42, 2), Ok(Confirmation::Confirmed));
assert_eq!(outbox.confirm(42, 2), Ok(Confirmation::AlreadyConfirmed));
assert_eq!(outbox.forget(42), Ok(Forgetting::Forgotten));
assert_eq!(outbox.forget(42), Ok(Forgetting::NotHeld));
assert_eq!(outbox.confirm(42, 2), Err(Refusal::Unknown));

// The id is free again, but the identities its attempts used are not.
assert_eq!(outbox.enqueue(42, Duration::from_secs(5)), Ok(Enqueue::Enqueued));
let anew = outbox.claim(Duration::from_secs(5), Duration::from_secs(30)).unwrap().unwrap();
assert_eq!((anew.attempt, outbox.get(42).unwrap().attempts), (3, 1));
assert_eq!(outbox.confirm(42, 1), Err(Refusal::Stale));
assert_eq!(outbox.confirm(42, 2), Err(Refusal::Stale));
```
