# frozen_string_literal: true

require_relative 'outbox_machines/version'

module OutboxMachines
  # A successful claim: the record id, the attempt identity that must
  # accompany `confirm` and `fail`, and the instant (Integer nanoseconds) from
  # which `expire_claims` may reap the lease. The outbox issues an attempt
  # identity once and never again, also after the record is forgotten and its
  # id enqueued anew; it is not the record's attempt count (`Entry#attempts`).
  Claimed = Data.define(:id, :attempt, :lease_expires)

  # The core's `Disposition::Retry(at)`: the record is pending again and
  # claimable at or after the instant `at` (Integer nanoseconds). The other
  # disposition, `Disposition::Dead`, is the symbol :dead.
  Retry = Data.define(:at)

  # The core's `Failure::Failed(disposition)`: `fail` ended the live attempt and
  # the record went where the disposition (a Retry or :dead) says.
  Failed = Data.define(:disposition)

  # The core's `Failure::AlreadyFailed(disposition)`: an idempotent repeat.
  # `fail` had already ended this same attempt with this same `retry_after`,
  # nothing changed, and the disposition is the standing one from the first
  # call.
  AlreadyFailed = Data.define(:disposition)

  # One claim reaped by `expire_claims`: the record id, the identity of the
  # attempt whose lease ran out, and what became of the record (a Retry or
  # :dead). Its outcome is unknown, not failed.
  Expired = Data.define(:id, :attempt, :disposition)

  # Progress of one bounded `expire_claims` call: the Expired values it reaped,
  # the slots it examined, and whether the cursor has completed a full pass.
  Sweep = Data.define(:expired, :scanned, :complete)

  # A read-only view of one record, from `Outbox#get`: its id, its state
  # (:pending, :claimed, :confirmed or :dead), the claims issued to it so far,
  # the identity of the latest one (nil until it is first claimed), and the
  # earliest instant (Integer nanoseconds) a pending record may be claimed.
  Entry = Data.define(:id, :state, :attempts, :attempt, :available_at)
end

# Native is required: no algorithm fallback. It builds the Data values above,
# so they must exist first.
require 'outbox_machines_native/outbox_machines_native'
