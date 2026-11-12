# frozen_string_literal: true

require 'minitest/autorun'
require 'outbox_machines'

class OutboxTest < Minitest::Test
  def outbox(max = 3) = OutboxMachines::Outbox.new(max)

  def test_attempt_is_data
    assert_operator OutboxMachines::Attempt, :<, Data
    attempt = outbox.tap { _1.enqueue(1, 0) }.claim(0, 10)
    assert_instance_of OutboxMachines::Attempt, attempt
    assert_equal [1, 1], [attempt.id, attempt.generation]
    assert_predicate attempt, :frozen?
  end

  def test_lifecycle_confirm
    box = outbox
    assert_equal :enqueued, box.enqueue(7, 0)
    assert_equal :duplicate, box.enqueue(7, 0)
    assert_equal :pending, box.state(7)
    attempt = box.claim(0, 10)
    assert_equal :claimed, box.state(7)
    assert_nil box.claim(0, 10)
    assert_equal :confirmed, box.confirm(attempt.id, attempt.generation)
    assert_equal :stale, box.confirm(attempt.id, attempt.generation)
    assert_equal :unknown, box.confirm(99, 1)
    assert_equal 1, box.count(:confirmed)
    assert_equal :forgotten, box.forget(7)
    assert_equal :unknown, box.forget(7)
    assert_equal 0, box.size
  end

  def test_fail_retry_then_dead
    box = outbox(2)
    box.enqueue(1, 0)
    first = box.claim(0, 10)
    assert_equal [:retry, 55], box.fail(1, first.generation, 5, 50)
    assert_nil box.claim(54, 10)
    second = box.claim(55, 10)
    assert_equal 2, second.generation
    assert_equal :stale, box.fail(1, first.generation, 56, 0)
    assert_equal :dead, box.fail(1, second.generation, 56, 0)
    assert_equal :dead, box.state(1)
    assert_equal :not_terminal, outbox.tap { _1.enqueue(2, 0) }.forget(2)
  end

  def test_expire_claims_resumable
    box = outbox(1)
    20.times { |i| box.enqueue(i, 0) }
    assert_equal 20, box.size
    20.times { box.claim(0, 10) }
    seen = []
    loop do
      rows, complete = box.expire_claims(10, 5)
      seen.concat(rows.map(&:first))
      assert_equal [:dead], rows.map(&:last).uniq if rows.any?
      break if complete
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
    assert_equal [[], true], box.expire_claims(9, 64)
    # Past the deadline but not yet reaped: the effect happened, so it counts.
    assert_nil box.state(3)
    assert_equal :enqueued, box.enqueue(3, 50)
    assert_equal :confirmed, box.confirm(second.id, second.generation)
    assert_equal [[[1, 1, [:retry, 50]]], true], box.expire_claims(50, 64)
    assert_equal :stale, box.confirm(first.id, first.generation)
    assert_equal 2, box.claim(50, 10).generation
  end

  def test_capacity_and_refusals
    box = outbox
    assert_equal 64, OutboxMachines::CAPACITY
    assert_equal OutboxMachines::CAPACITY, box.capacity
    assert_equal 3, box.max_attempts
    64.times { |i| assert_equal :enqueued, box.enqueue(i, 0) }
    assert_equal :full, box.enqueue(100, 0)
    assert_equal :invalid_duration, box.claim(0, 0)
    box.enqueue(0, 5)
    assert_equal :time_regression, box.enqueue(200, 4)
  end

  def test_strict_inputs
    box = outbox
    [1.0, nil, '1', Object.new, Float::NAN].each do |bad|
      assert_raises(TypeError) { box.enqueue(bad, 0) }
      assert_raises(TypeError) { box.claim(bad, 1) }
      assert_raises(TypeError) { box.fail(1, 1, 0, bad) }
    end
    [-1, 2**64].each { |n| assert_raises(RangeError) { box.enqueue(n, 0) } }
    [0, 2**32].each { |n| assert_raises(RangeError) { outbox(n) } }
    assert_raises(ArgumentError) { box.count(:nope) }
    assert_raises(TypeError) { OutboxMachines::Outbox.allocate }
    assert_equal :stale, box.confirm(1, 2**40)
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
    assert_nil box.state(1)
    assert_equal 0, box.count(:pending)
    assert_equal 64, box.capacity
  end

  def test_gc_stress
    previous = GC.stress
    GC.stress = true
    box = outbox
    box.enqueue(2**64 - 1, 0)
    attempt = box.claim(0, 1)
    assert_equal 2**64 - 1, attempt.id
    assert_equal :confirmed, box.confirm(attempt.id, attempt.generation)
  ensure
    GC.stress = previous
  end
end
