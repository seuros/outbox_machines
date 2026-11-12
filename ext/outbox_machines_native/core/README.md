# outbox-machines

> **AMS Helland**, NCC-4221, *Helland*-class dispatch courier.

Engine-room flight manual: track owed dispatches through generation-checked attempts.

Initial, unstable. Bounded delivery-policy metadata for an outbox: records move
through `pending`, `claimed`, `confirmed` and `dead`, and every attempt carries
a generation so a stale worker cannot touch a newer attempt.

This is **delivery policy only**. It is not a transactional outbox or inbox, not
a relay, and not a retry engine: it never sleeps, schedules, or performs I/O.
Atomic insertion with business writes, durable claims, crash recovery and
payload storage are adapter responsibilities. It composes with saga, dedup,
lease and chrono through explicit caller-side contracts only; this crate has no
dependency on any sibling.

Allocation-free `no_std`. Capacity `N` is a const generic; a full table answers
`Enqueue::Full` and never evicts. Time is explicit (`now` is a `Duration` the
caller passes) and must not go backwards. Lookup is a linear scan, so keep `N`
small. Ids are `u64`; payloads live outside.

- `enqueue(id, now)`: `Enqueued`, `Full` or `Duplicate`. An id stays taken,
  even when confirmed or dead, until `forget(id)`.
- `claim(now, lease)`: the due pending record enqueued first, or `None`. The
  attempt generation is the 1-based claim count.
- `confirm(id, generation)`: the caller asserts the external effect was
  persisted. The outbox cannot verify it. Accepted even after the lease
  deadline, as long as no newer attempt exists.
- `fail(id, generation, now, retry_after)`: a *known* failure. Back to pending
  at `now + retry_after`, or `Dead` once `max_attempts` claims are spent.
- `expire_claims(now, max_scan, out)`: reaps claims whose lease ended, into a
  caller buffer, with a persistent cursor. Bounded by `max_scan` and
  `out.len()`; call again until `Sweep::complete`.
- `forget(id)`: drops a confirmed or dead record.

A wrong or superseded generation, or a record that is not claimed, is
`Error::Stale`. Refusals never mutate the table.

## Ambiguous effects

An expired lease is an unknown outcome, not a failure: the worker may still run
or may have already produced the effect. Expired claims are reported separately
so the adapter can reconcile (fence, query the destination, or use an
idempotency key) before trusting a retry. Use `fail` only for failures you know
did not take effect.

```rust
use core::num::NonZeroU32;
use core::time::Duration;
use outbox_machines::{Disposition, Enqueue, Outbox};

let mut outbox = Outbox::<8>::new(NonZeroU32::new(3).unwrap());
assert_eq!(outbox.enqueue(42, Duration::ZERO), Ok(Enqueue::Enqueued));
let claimed = outbox
    .claim(Duration::ZERO, Duration::from_secs(30))
    .unwrap()
    .unwrap();
assert_eq!((claimed.id, claimed.generation), (42, 1));
assert_eq!(
    outbox.fail(42, 1, Duration::ZERO, Duration::from_secs(5)),
    Ok(Disposition::Retry(Duration::from_secs(5)))
);
let retry = outbox.claim(Duration::from_secs(5), Duration::from_secs(30)).unwrap().unwrap();
assert_eq!(retry.generation, 2);
assert_eq!(outbox.confirm(42, 1), Err(outbox_machines::Error::Stale));
assert_eq!(outbox.confirm(42, 2), Ok(()));
```
