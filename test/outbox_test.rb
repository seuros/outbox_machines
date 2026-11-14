# frozen_string_literal: true

require 'minitest/autorun'
require 'outbox_machines'

class OutboxTest < Minitest::Test
  def outbox(max = 3) = OutboxMachines::Outbox.new(max)

  Claimed = OutboxMachines::Claimed
  Expired = OutboxMachines::Expired
  Sweep = OutboxMachines::Sweep
  Retry = OutboxMachines::Retry
  Entry = OutboxMachines::Entry
  Failed = OutboxMachines::Failed
  AlreadyFailed = OutboxMachines::AlreadyFailed

  def test_values_mirror_the_core
    [Claimed, Expired, Sweep, Retry, Entry, Failed, AlreadyFailed].each { assert_operator _1, :<, Data }
    assert_equal [:disposition], Failed.members
    assert_equal [:disposition], AlreadyFailed.members
    assert_equal [:at], Retry.members
    assert_equal %i[id state attempts attempt available_at], Entry.members
    assert_equal %i[id attempt lease_expires], Claimed.members
    assert_equal %i[id attempt disposition], Expired.members
    assert_equal %i[expired scanned pass], Sweep.members
    refute OutboxMachines.const_defined?(:Attempt)
    box = outbox.tap { _1.enqueue(1, 0) }
    claimed = box.claim(0, 10)
    assert_equal Claimed.new(1, 1, 10), claimed
    assert_predicate claimed, :frozen?
    sweep = box.expire_claims(10, 3)
    assert_equal Sweep.new([Expired.new(1, 1, Retry.new(10))], 3, :partial), sweep
    assert_predicate sweep, :frozen?
    assert_predicate sweep.expired, :frozen?
    assert_equal Sweep.new([], 61, :complete), box.expire_claims(10, 64)
  end

  def test_times_are_integer_nanoseconds
    now = Process.clock_gettime(Process::CLOCK_MONOTONIC, :nanosecond)
    lease = 30 * 1_000_000_000
    box = outbox
    assert_equal :enqueued, box.enqueue(1, now)
    claimed = box.claim(now, lease)
    assert_equal now + lease, claimed.lease_expires
    assert_equal Failed.new(Retry.new(now + 7)), box.fail(1, claimed.attempt, now + 2, 5)
    assert_nil box.claim(now + 6, lease)
    retried = box.claim(now + 7, lease)
    assert_equal Sweep.new([], 64, :complete), box.expire_claims(retried.lease_expires - 1, 64)
    reaped = Expired.new(1, 2, Retry.new(retried.lease_expires))
    assert_equal Sweep.new([reaped], 64, :complete), box.expire_claims(retried.lease_expires, 64)
  end

  def test_instants_past_u64_are_answered_exactly
    top = 2**64 - 1
    box = outbox
    box.enqueue(1, top)
    claimed = box.claim(top, top)
    assert_equal 2 * top, claimed.lease_expires
    assert_equal Failed.new(Retry.new(2 * top)), box.fail(1, claimed.attempt, top, top)
    assert_nil box.claim(top, 1)
  end

  def test_get_answers_an_entry
    box = outbox(2)
    assert_predicate box, :empty?
    assert_nil box.get(1)
    box.enqueue(1, 3)
    refute_predicate box, :empty?
    assert_equal Entry.new(1, :pending, 0, nil, 3), box.get(1)
    claimed = box.claim(4, 10)
    assert_equal Entry.new(1, :claimed, 1, claimed.attempt, 3), box.get(1)
    assert_equal Failed.new(Retry.new(12)), box.fail(1, claimed.attempt, 5, 7)
    assert_equal Entry.new(1, :pending, 1, claimed.attempt, 12), box.get(1)
    assert_predicate box.get(1), :frozen?
    assert_raises(TypeError) { box.get(nil) }
    assert_raises(NoMethodError) { box.state(1) }
  end

  def test_lifecycle_confirm
    box = outbox
    assert_equal :enqueued, box.enqueue(7, 0)
    assert_equal :duplicate, box.enqueue(7, 0)
    assert_equal :pending, box.get(7).state
    claimed = box.claim(0, 10)
    assert_equal :claimed, box.get(7).state
    assert_nil box.claim(0, 10)
    assert_equal :confirmed, box.confirm(claimed.id, claimed.attempt)
    assert_equal :already_confirmed, box.confirm(claimed.id, claimed.attempt)
    assert_equal :stale, box.confirm(claimed.id, claimed.attempt + 1)
    assert_equal :stale, box.fail(claimed.id, claimed.attempt, 0, 0)
    assert_equal :unknown, box.confirm(99, 1)
    assert_equal 1, box.count(:confirmed)
    assert_equal :forgotten, box.forget(7)
    assert_equal :not_held, box.forget(7)
    assert_equal :not_held, box.forget(99)
    assert_equal :unknown, box.confirm(claimed.id, claimed.attempt)
    assert_equal 0, box.size
  end

  def test_repeats_are_honoured_and_change_nothing
    box = outbox(2)
    box.enqueue(1, 0)
    first = box.claim(0, 10)
    assert_equal Failed.new(Retry.new(55)), box.fail(1, first.attempt, 5, 50)
    entry = box.get(1)
    assert_equal AlreadyFailed.new(Retry.new(55)), box.fail(1, first.attempt, 5, 50)
    # Another instant is the same report made later; 900 is not observed.
    repeat = box.fail(1, first.attempt, 900, 50)
    assert_equal AlreadyFailed.new(Retry.new(55)), repeat
    assert_predicate repeat, :frozen?
    # Another delay is another report, even one landing on the same instant.
    assert_equal :mismatch, box.fail(1, first.attempt, 900, 7)
    assert_equal :mismatch, box.fail(1, first.attempt, 6, 49)
    # The arguments are checked before the payload is.
    assert_equal :time_regression, box.fail(1, first.attempt, 4, 50)
    assert_equal :time_regression, box.fail(1, first.attempt, 4, 7)
    assert_equal :stale, box.confirm(1, first.attempt)
    assert_equal :not_held, box.forget(2)
    assert_equal entry, box.get(1)
    assert_nil box.claim(54, 10)
    second = box.claim(55, 10)
    assert_equal 2, second.attempt
    assert_equal :stale, box.fail(1, first.attempt, 55, 0)
    assert_equal Failed.new(:dead), box.fail(1, second.attempt, 56, 0)
    assert_equal :dead, box.get(1).state
    assert_equal AlreadyFailed.new(:dead), box.fail(1, second.attempt, 999, 0)
    assert_equal :mismatch, box.fail(1, second.attempt, 999, 3)
    # 999 was not observed either: 56 is still a valid instant.
    assert_equal 0, box.expire_claims(56, 0).scanned
  end

  def test_a_reaped_attempt_was_not_failed
    box = outbox(1)
    box.enqueue(1, 0)
    lost = box.claim(0, 5)
    assert_equal [Expired.new(1, 1, :dead)], box.expire_claims(5, 64).expired
    assert_equal :stale, box.fail(1, lost.attempt, 5, 0)
    assert_equal :stale, box.confirm(1, lost.attempt)
  end

  def test_expire_claims_resumable
    box = outbox(1)
    20.times { |i| box.enqueue(i, 0) }
    assert_equal 20, box.size
    20.times { box.claim(0, 10) }
    seen = []
    loop do
      sweep = box.expire_claims(10, 5)
      assert_operator sweep.scanned, :<=, 5
      seen.concat(sweep.expired.map(&:id))
      assert_equal [:dead], sweep.expired.map(&:disposition).uniq if sweep.expired.any?
      break if sweep.pass == :complete
    end
    assert_equal (0...20).to_a, seen.sort
    assert_equal 20, box.count(:dead)
    assert_equal :time_regression, box.expire_claims(9, 1)
  end

  def test_expired_lease_is_reported_and_late_confirm_rules
    box = outbox(2)
    box.enqueue(1, 0)
    box.enqueue(2, 0)
    first = box.claim(0, 10)
    second = box.claim(0, 10)
    assert_equal Sweep.new([], 64, :complete), box.expire_claims(9, 64)
    # Past the deadline but not yet reaped: the effect happened, so it counts.
    assert_nil box.get(3)
    assert_equal :enqueued, box.enqueue(3, 50)
    assert_equal :confirmed, box.confirm(second.id, second.attempt)
    assert_equal Sweep.new([Expired.new(1, 1, Retry.new(50))], 64, :complete), box.expire_claims(50, 64)
    assert_equal :stale, box.confirm(first.id, first.attempt)
    # The third claim this outbox issued, and the second for this record.
    assert_equal 3, box.claim(50, 10).attempt
    assert_equal 2, box.get(1).attempts
  end

  def test_capacity_and_refusals_change_nothing
    box = outbox
    assert_equal 64, OutboxMachines::CAPACITY
    assert_equal OutboxMachines::CAPACITY, box.capacity
    assert_equal 3, box.max_attempts
    64.times { |i| assert_equal :enqueued, box.enqueue(i, 10) }
    assert_equal :full, box.enqueue(100, 500)
    assert_equal :duplicate, box.enqueue(0, 500)
    assert_equal :time_regression, box.enqueue(0, 9)
    assert_equal :invalid_duration, box.claim(10, 0)
    assert_equal :not_terminal, box.forget(0)
    assert_equal 64, box.size
    # None of those calls recorded 500 as observed: 10 is still a valid instant.
    assert_equal 0, box.claim(10, 5).id
  end

  def test_an_honoured_call_records_the_instant_even_with_nothing_to_do
    box = outbox
    assert_nil box.claim(20, 5)
    assert_equal :time_regression, box.enqueue(1, 19)
    assert_equal Sweep.new([], 64, :complete), box.expire_claims(30, 64)
    assert_equal :time_regression, box.enqueue(1, 29)
    assert_equal :enqueued, box.enqueue(1, 30)
  end

  def test_strict_inputs
    box = outbox
    [1.0, nil, '1', Object.new, Float::NAN].each do |bad|
      assert_raises(TypeError) { box.enqueue(bad, 0) }
      assert_raises(TypeError) { box.claim(bad, 1) }
      assert_raises(TypeError) { box.fail(1, 1, 0, bad) }
    end
    [-1, 2**64].each { |n| assert_raises(RangeError) { box.enqueue(n, 0) } }
    assert_time_arguments_are_strict(box)
    [0, 2**32].each { |n| assert_raises(RangeError) { outbox(n) } }
    assert_raises(TypeError) { OutboxMachines::Outbox.allocate }
    # An attempt the outbox never issued is refused before the id is looked up.
    assert_equal :stale, box.confirm(1, 2**48 - 1)
    assert_equal :stale, box.fail(1, 2**48 - 1, 0, 0)
    box.enqueue(1, 0)
    assert_equal :stale, box.confirm(1, 2**48 - 1)
    assert_equal :stale, box.fail(1, 2**64 - 1, 0, 0)
    assert_raises(RangeError) { box.confirm(1, 2**64) }
    assert_raises(RangeError) { box.fail(1, -1, 0, 0) }
  end

  def assert_time_arguments_are_strict(box)
    calls = [
      ->(t) { box.enqueue(1, t) }, ->(t) { box.claim(t, 1) }, ->(t) { box.claim(0, t) },
      ->(t) { box.fail(1, 1, t, 0) }, ->(t) { box.fail(1, 1, 0, t) }, ->(t) { box.expire_claims(t, 1) }
    ]
    calls.each do |call|
      [1.0, 1r, nil, '1', Time.now].each { |bad| assert_raises(TypeError) { call.(bad) } }
      [-1, 2**64].each { |bad| assert_raises(RangeError) { call.(bad) } }
    end
    assert_equal 0, box.size
  end

  def test_freeze_guards
    box = outbox
    box.freeze
    assert_raises(FrozenError) { box.enqueue(1, 0) }
    assert_raises(FrozenError) { box.claim(0, 1) }
    assert_raises(FrozenError) { box.expire_claims(0, 1) }
    assert_raises(FrozenError) { box.confirm(1, 1) }
    assert_raises(FrozenError) { box.fail(1, 1, 0, 0) }
    assert_raises(FrozenError) { box.forget(1) }
    assert_nil box.get(1)
    assert_equal 0, box.count(:pending)
    assert_equal 64, box.capacity
  end

  def test_gc_stress
    previous = GC.stress
    GC.stress = true
    box = outbox
    box.enqueue(2**64 - 1, 0)
    claimed = box.claim(0, 1)
    assert_equal 2**64 - 1, claimed.id
    assert_equal :confirmed, box.confirm(claimed.id, claimed.attempt)
  ensure
    GC.stress = previous
  end
end
