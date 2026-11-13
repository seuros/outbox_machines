# frozen_string_literal: true

require 'minitest/autorun'
require 'outbox_machines'

# Attempt identities are never reissued, and a repeat must match its record.
class ReissueTest < Minitest::Test
  Failed = OutboxMachines::Failed
  AlreadyFailed = OutboxMachines::AlreadyFailed
  Retry = OutboxMachines::Retry

  def test_a_report_from_a_forgotten_life_cannot_settle_the_new_one
    box = OutboxMachines::Outbox.new(3)
    box.enqueue(7, 0)
    old = box.claim(0, 10)
    assert_equal :confirmed, box.confirm(7, old.attempt)
    assert_equal :forgotten, box.forget(7)
    assert_equal :enqueued, box.enqueue(7, 0)
    assert_equal :stale, box.confirm(7, old.attempt)
    fresh = box.claim(0, 10)
    refute_equal old.attempt, fresh.attempt
    entry = box.get(7)
    assert_equal [1, fresh.attempt], [entry.attempts, entry.attempt]
    assert_equal :stale, box.confirm(7, old.attempt)
    assert_equal :stale, box.fail(7, old.attempt, 0, 0)
    assert_equal entry, box.get(7)
    assert_equal :confirmed, box.confirm(7, fresh.attempt)
  end

  def test_a_late_fail_from_a_dead_life_cannot_fail_the_new_one
    box = OutboxMachines::Outbox.new(1)
    box.enqueue(7, 0)
    old = box.claim(0, 10)
    assert_equal Failed.new(:dead), box.fail(7, old.attempt, 0, 4)
    box.forget(7)
    box.enqueue(7, 0)
    fresh = box.claim(0, 10)
    assert_equal :stale, box.fail(7, old.attempt, 0, 4)
    assert_equal :claimed, box.get(7).state
    assert_equal Failed.new(:dead), box.fail(7, fresh.attempt, 0, 4)
  end

  def test_a_repeat_is_honoured_only_for_the_attempt_that_produced_the_outcome
    box = OutboxMachines::Outbox.new(1)
    olds = settle_both(box)
    assert_equal [:forgotten, :forgotten], [box.forget(7), box.forget(8)]
    confirmed, failed = settle_both(box)
    assert_equal :already_confirmed, box.confirm(7, confirmed)
    assert_equal AlreadyFailed.new(:dead), box.fail(8, failed, 0, 0)
    olds.each do |old|
      assert_equal :stale, box.confirm(7, old)
      assert_equal :stale, box.fail(8, old, 0, 0)
    end
  end

  def test_a_repeated_fail_with_another_retry_after_is_a_mismatch
    box = OutboxMachines::Outbox.new(3)
    box.enqueue(1, 0)
    first = box.claim(0, 10)
    assert_equal Failed.new(Retry.new(55)), box.fail(1, first.attempt, 5, 50)
    entry = box.get(1)
    assert_equal :mismatch, box.fail(1, first.attempt, 900, 7)
    assert_equal :mismatch, box.fail(1, first.attempt, 6, 49)
    assert_equal :time_regression, box.fail(1, first.attempt, 4, 7)
    assert_equal AlreadyFailed.new(Retry.new(55)), box.fail(1, first.attempt, 900, 50)
    assert_equal entry, box.get(1)
    # 900 was never observed: 55 is still a valid instant.
    assert_equal 1, box.claim(55, 10).id
  end

  private

  # Enqueue ids 7 and 8, confirm 7 and fail 8 dead; the two attempts.
  def settle_both(box)
    box.enqueue(7, 0)
    box.enqueue(8, 0)
    seven = box.claim(0, 10)
    eight = box.claim(0, 10)
    assert_equal :confirmed, box.confirm(7, seven.attempt)
    assert_equal Failed.new(:dead), box.fail(8, eight.attempt, 0, 0)
    [seven.attempt, eight.attempt]
  end
end
