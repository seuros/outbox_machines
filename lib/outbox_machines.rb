# frozen_string_literal: true

require_relative 'outbox_machines/version'

module OutboxMachines
  # A successful claim: the record id, the attempt identity that must
  # accompany `confirm` and `fail`, and the instant (Integer nanoseconds) from
  # which `expire_claims` may reap the lease. The outbox issues an attempt
  # identity once and never again, also after the record is forgotten and its
  # id enqueued anew; it is not the record's attempt count (`Entry#attempts`).
  # The identity carries the scope of the outbox that issued it
  # (`Outbox.with_scope`); an outbox of another scope answers :stale to it.
  #
  # Built by hand it is held to the core's `Claimed::new`: Integers in
  # 0..2**64-1, an attempt whose sequence (low 48 bits) is at least 1, and a
  # positive `lease_expires`. TypeError for the wrong type, RangeError for a
  # value outside that.
  Claimed = Data.define(:id, :attempt, :lease_expires) do
    def initialize(id:, attempt:, lease_expires:)
      OutboxMachines.__send__(:__claimed, id, attempt, lease_expires)
      super
    end

    # The scope of the outbox that issued this attempt.
    def scope = Outbox.__send__(:__scope_of, attempt)
  end

  # The core's `Disposition::Retry(at)`: the record is pending again and
  # claimable at or after the instant `at` (Integer nanoseconds). The other
  # disposition, `Disposition::Dead`, is the symbol :dead.
  # `at` is an Integer: TypeError otherwise, RangeError when negative or past
  # what a `Duration` holds.
  Retry = Data.define(:at) do
    def initialize(at:)
      OutboxMachines.__send__(:__retry, at)
      super
    end
  end

  # The core's `Failure::Failed(disposition)`: `fail` ended the live attempt and
  # the record went where the disposition (a Retry or :dead) says.
  # A disposition is a Retry or :dead: TypeError for anything else that is not
  # a Symbol, RangeError for another Symbol.
  Failed = Data.define(:disposition) do
    def initialize(disposition:)
      OutboxMachines.__send__(:__failure, disposition)
      super
    end
  end

  # The core's `Failure::AlreadyFailed(disposition)`: an idempotent repeat.
  # `fail` had already ended this same attempt with this same `retry_after`,
  # nothing changed, and the disposition is the standing one from the first
  # call.
  # The disposition is checked as Failed's is.
  AlreadyFailed = Data.define(:disposition) do
    def initialize(disposition:)
      OutboxMachines.__send__(:__failure, disposition)
      super
    end
  end

  # One claim reaped by `expire_claims`: the record id, the identity of the
  # attempt whose lease ran out, and what became of the record (a Retry or
  # :dead). Its outcome is unknown, not failed.
  # Built by hand it is held to the core's `Expired::new`: an attempt whose
  # sequence is at least 1, a disposition as Failed takes.
  Expired = Data.define(:id, :attempt, :disposition) do
    def initialize(id:, attempt:, disposition:)
      OutboxMachines.__send__(:__expired, id, attempt, disposition)
      super
    end

    # The scope of the outbox that issued the reaped attempt.
    def scope = Outbox.__send__(:__scope_of, attempt)
  end

  # Progress of one bounded `expire_claims` call: the Expired values it reaped,
  # the slots it examined, and where the cursor stands: the core's `Pass`,
  # :complete once it has finished a full pass over every slot (cumulatively
  # across calls), :partial until then.
  # Built by hand it is held to the core's `Sweep::new`: a plain Array (no subclass) of Expired
  # (kept as a frozen copy) no longer than `scanned`, and a `pass` of :partial
  # or :complete (TypeError for a non-Symbol, RangeError for another Symbol).
  Sweep = Data.define(:expired, :scanned, :pass) do
    def initialize(expired:, scanned:, pass:)
      OutboxMachines.__send__(:__sweep, expired, scanned, pass)
      super(expired: expired.frozen? ? expired : expired.dup.freeze, scanned:, pass:)
    end
  end

  # A read-only view of one record, from `Outbox#get`: its id, its state
  # (:pending, :claimed, :confirmed or :dead), the claims issued to it so far,
  # the identity of the latest one (nil until it is first claimed), and the
  # earliest instant (Integer nanoseconds) a pending record may be claimed.
  #
  # Built by hand it is held to the core's `Entry::new`: `attempt` is nil
  # exactly when `attempts` is 0, a present one has a sequence of at least 1,
  # and only a :pending record is unclaimed. TypeError for the wrong type,
  # RangeError for a value outside that.
  Entry = Data.define(:id, :state, :attempts, :attempt, :available_at) do
    def initialize(id:, state:, attempts:, attempt:, available_at:)
      OutboxMachines.__send__(:__entry, id, state, attempts, attempt, available_at)
      super
    end
  end
end

# Native is required: no algorithm fallback. It builds and validates the Data
# values above, so they must exist first.
require 'outbox_machines_native/outbox_machines_native'

module OutboxMachines
  private_class_method :__retry, :__failure, :__claimed, :__expired, :__sweep, :__entry

  # The values are final. `initialize` is where each validates, and a subclass
  # could override it, or `allocate` build an instance that never ran it:
  # subclassing raises TypeError and `allocate` is gone. That stops accidents,
  # not reflection (Ruby binds a named subclass's constant before `inherited`
  # raises; `Marshal.load` and a hand-bound `Data#initialize` skip the check),
  # so the extension is the authority: every value it reads back must be
  # exactly its class and is checked again, TypeError or RangeError otherwise.
  [Claimed, Retry, Failed, AlreadyFailed, Expired, Sweep, Entry].each do |value|
    value.singleton_class.undef_method(:allocate)
    value.define_singleton_method(:inherited) { |_| raise TypeError, "#{value} cannot be subclassed" }
  end

  class Outbox
    private_class_method :__scope_of
  end
end
