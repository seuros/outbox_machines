# frozen_string_literal: true

require 'minitest/autorun'
require 'outbox_machines'

# Values are valid or do not exist: a hand-built one is held to the rule the
# core holds its own to. Wrong type is TypeError, a value the core's
# constructor answers None for is RangeError.
class ValuesTest < Minitest::Test
  include OutboxMachines

  TOP = (((2**64) - 1) * 1_000_000_000) + 999_999_999 # Duration::MAX in nanoseconds

  def test_retry_holds_an_instant
    assert_equal 5, Retry.new(5).at
    assert_equal TOP, Retry.new(at: TOP).at
    [-1, TOP + 1].each { |bad| assert_raises(RangeError) { Retry.new(bad) } }
    [1.0, nil, '1', :dead].each { |bad| assert_raises(TypeError) { Retry.new(bad) } }
  end

  def test_failures_hold_a_disposition
    [Failed, AlreadyFailed].each do |failure|
      assert_equal :dead, failure.new(:dead).disposition
      assert_equal Retry.new(5), failure.new(disposition: Retry.new(5)).disposition
      %i[nope retry].each do |bad|
        assert_equal 'disposition must be a Retry or :dead', assert_raises(RangeError) { failure.new(bad) }.message
      end
      [nil, 5, 'dead', Object.new, Claimed.new(1, 1, 1)].each { |bad| assert_raises(TypeError) { failure.new(bad) } }
    end
  end

  def test_claimed_needs_an_issuable_attempt_and_a_lease_end
    assert_equal [7, 1, 10], Claimed.new(7, 1, 10).deconstruct
    assert_equal 9, Claimed.new(7, (9 << 48) | 3, TOP).scope
    assert_raises(RangeError) { Claimed.new(7, 0, 10) }
    assert_raises(RangeError) { Claimed.new(7, 9 << 48, 10) }
    assert_raises(RangeError) { Claimed.new(7, 1, 0) }
    assert_raises(RangeError) { Claimed.new(-1, 1, 10) }
    assert_raises(RangeError) { Claimed.new(7, 2**64, 10) }
    assert_raises(RangeError) { Claimed.new(7, 1, TOP + 1) }
    assert_raises(TypeError) { Claimed.new(nil, 1, 10) }
    assert_raises(TypeError) { Claimed.new(7, 1.0, 10) }
    assert_raises(TypeError) { Claimed.new(7, 1, '10') }
  end

  def test_expired_needs_an_issuable_attempt_and_a_disposition
    assert_equal [7, 2, :dead], Expired.new(7, 2, :dead).deconstruct
    assert_equal Retry.new(4), Expired.new(7, 2, Retry.new(4)).disposition
    assert_raises(RangeError) { Expired.new(7, 0, :dead) }
    assert_raises(RangeError) { Expired.new(7, 2, :nope) }
    assert_raises(RangeError) { Expired.new(2**64, 2, :dead) }
    assert_raises(TypeError) { Expired.new(7, nil, :dead) }
    assert_raises(TypeError) { Expired.new(7, 2, 4) }
  end

  def test_sweep_holds_expired_values_it_scanned
    reaped = [Expired.new(1, 1, :dead)]
    sweep = Sweep.new(reaped, 3, :partial)
    assert_equal [reaped, 3, :partial], sweep.deconstruct
    assert_predicate sweep.expired, :frozen?
    refute_predicate reaped, :frozen?
    assert_equal Sweep.new([], 0, :complete), Sweep.new(expired: [], scanned: 0, pass: :complete)
    [[reaped, 0, :complete], [[], -1, :complete], [[], 3, :nope]].each do |bad|
      assert_raises(RangeError) { Sweep.new(*bad) }
    end
    [nil, [1], [Claimed.new(1, 1, 1)]].each { |bad| assert_raises(TypeError) { Sweep.new(bad, 3, :complete) } }
    assert_raises(TypeError) { Sweep.new([], 3.0, :complete) }
    [nil, true, 'complete'].each { |bad| assert_raises(TypeError) { Sweep.new([], 3, bad) } }
  end

  def test_entry_agrees_with_itself
    assert_equal [1, :pending, 0, nil, 3], Entry.new(1, :pending, 0, nil, 3).deconstruct
    assert_equal 5, Entry.new(1, :pending, 2, 5, TOP).attempt
    %i[claimed confirmed dead].each do |state|
      assert_equal state, Entry.new(1, state, 1, 1, 3).state
      assert_raises(RangeError) { Entry.new(1, state, 0, nil, 3) }
    end
    assert_raises(RangeError) { Entry.new(1, :pending, 1, nil, 3) }
    assert_raises(RangeError) { Entry.new(1, :pending, 0, 1, 3) }
    assert_raises(RangeError) { Entry.new(1, :claimed, 1, 0, 3) }
    assert_raises(RangeError) { Entry.new(1, :nope, 0, nil, 3) }
    assert_raises(RangeError) { Entry.new(1, :pending, 2**32, 1, 3) }
    assert_raises(RangeError) { Entry.new(1, :pending, 0, nil, -3) }
    assert_raises(TypeError) { Entry.new(1, 'pending', 0, nil, 3) }
    assert_raises(TypeError) { Entry.new(1, :pending, nil, nil, 3) }
    assert_raises(TypeError) { Entry.new(1, :pending, 1, 1.0, 3) }
    assert_raises(TypeError) { Entry.new('1', :pending, 0, nil, 3) }
  end

  def test_the_hooks_are_private
    %i[__retry __failure __claimed __expired __sweep __entry].each do |hook|
      assert_raises(NoMethodError) { OutboxMachines.public_send(hook) }
    end
  end

  def test_count_names_a_state
    box = Outbox.new(3)
    assert_raises(RangeError) { box.count(:nope) }
    ['pending', nil, 1].each { |bad| assert_raises(TypeError) { box.count(bad) } }
  end
end
