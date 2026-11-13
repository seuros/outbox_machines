# frozen_string_literal: true

require 'minitest/autorun'
require 'outbox_machines'

# Attempt identities carry the scope of the outbox that issued them.
class ScopeTest < Minitest::Test
  Outbox = OutboxMachines::Outbox
  AlreadyFailed = OutboxMachines::AlreadyFailed
  Retry = OutboxMachines::Retry

  # Ids 7 and 8 enqueued; the claim on 7 confirmed, the claim on 8 failed, and
  # a live claim on 9.
  def settled(scope)
    box = Outbox.with_scope(3, scope)
    [7, 8].each { |id| box.enqueue(id, 0) }
    confirmed = box.claim(0, 10)
    failed = box.claim(0, 10)
    box.confirm(7, confirmed.attempt)
    box.fail(8, failed.attempt, 1, 4)
    box.enqueue(9, 1)
    [box, confirmed, failed, box.claim(1, 10)]
  end

  def entries(box) = [7, 8, 9].map { |id| box.get(id) }

  def test_new_is_scope_zero_and_issues_the_plain_counter
    box = Outbox.new(3)
    assert_equal 0, box.scope
    2.times { |id| box.enqueue(id, 0) }
    first = box.claim(0, 5)
    assert_equal [1, 0, 2], [first.attempt, first.scope, box.claim(0, 5).attempt]
  end

  def test_with_scope_stamps_every_attempt
    box = Outbox.with_scope(3, 9)
    assert_equal [9, 3], [box.scope, box.max_attempts]
    box.enqueue(1, 0)
    claimed = box.claim(0, 5)
    assert_equal [(9 << 48) | 1, 9], [claimed.attempt, claimed.scope]
    assert_equal claimed.attempt, box.get(1).attempt
    expired = box.expire_claims(5, 64).expired
    assert_equal [[claimed.attempt, 9]], expired.map { [it.attempt, it.scope] }
    assert_raises(NoMethodError) { Outbox.__scope_of(1) }
    assert_equal 65_535, Outbox.with_scope(1, 65_535).scope
  end

  def test_an_attempt_from_another_outbox_settles_nothing
    near, *near_issued = settled(1)
    far, *far_issued = settled(2)
    [[near, far_issued], [far, near_issued]].each do |box, (confirmed, failed, live)|
      before = entries(box)
      assert_equal :stale, box.confirm(9, live.attempt)
      assert_equal :stale, box.fail(9, live.attempt, 1, 4)
      assert_equal :stale, box.confirm(7, confirmed.attempt)
      assert_equal :stale, box.fail(8, failed.attempt, 1, 4)
      assert_equal :stale, box.confirm(404, live.attempt)
      assert_equal :stale, box.fail(404, live.attempt, 1, 4)
      assert_equal before, entries(box)
    end
    assert_equal :confirmed, near.confirm(9, near_issued[2].attempt)
    assert_equal :already_confirmed, near.confirm(7, near_issued[0].attempt)
    assert_equal AlreadyFailed.new(Retry.new(5)), far.fail(8, far_issued[1].attempt, 1, 4)
  end

  def test_two_outboxes_left_in_one_scope_still_alias
    one, = settled(5)
    _other, other_confirmed, _failed, other_live = settled(5)
    assert_equal :confirmed, one.confirm(9, other_live.attempt)
    assert_equal :already_confirmed, one.confirm(7, other_confirmed.attempt)
  end

  def test_an_attempt_never_issued_is_refused_whatever_the_id
    box = Outbox.with_scope(3, 3)
    box.enqueue(1, 0)
    issued = box.claim(0, 10).attempt
    [(3 << 48) | 2, 3 << 48, (3 << 48) | (2**48 - 1)].each do |never|
      assert_equal :stale, box.confirm(404, never)
      assert_equal :stale, box.fail(404, never, 1, 4)
      assert_equal :stale, box.confirm(1, never)
    end
    assert_equal :unknown, box.confirm(404, issued)
    assert_equal :unknown, box.fail(404, issued, 1, 4)
    assert_equal [:claimed, issued], [box.get(1).state, box.get(1).attempt]
  end

  def test_resumed_continues_the_saved_sequence
    [0, 9, 65_535].each do |scope|
      first = Outbox.with_scope(3, scope)
      assert_equal 0, first.issued
      [7, 8].each { |id| first.enqueue(id, 0) }
      old = [first.claim(0, 10), first.claim(0, 10)]
      assert_equal 2, first.issued

      second = Outbox.resumed(3, first.scope, first.issued)
      assert_equal [scope, 2, 3, 0], [second.scope, second.issued, second.max_attempts, second.size]
      second.enqueue(7, 0)
      fresh = second.claim(0, 10)
      assert_equal [(scope << 48) | 3, 3], [fresh.attempt, second.issued]
      old.each { |late| assert_equal %i[stale unknown], [7, 8].map { |id| second.confirm(id, late.attempt) } }
      assert_equal :stale, second.confirm(8, (scope << 48) | 4)
      assert_equal :confirmed, second.confirm(7, fresh.attempt)
    end
  end

  def test_resumed_reaches_the_end_of_the_sequence
    box = Outbox.resumed(3, 65_535, 2**48 - 2)
    [1, 2].each { |id| box.enqueue(id, 0) }
    assert_equal 2**64 - 1, box.claim(0, 5).attempt
    assert_equal :exhausted, box.claim(0, 5)
    assert_equal [2**48 - 1, :pending], [box.issued, box.get(2).state]
  end

  def test_resumed_checks_its_arguments
    [2**48, 2**64 - 1].each { |past| assert_raises(RangeError) { Outbox.resumed(3, 0, past) } }
    [-1, 2**64].each { |bad| assert_raises(RangeError) { Outbox.resumed(3, 0, bad) } }
    [1.0, nil, '1'].each { |bad| assert_raises(TypeError) { Outbox.resumed(3, 0, bad) } }
    assert_raises(TypeError) { Outbox.resumed(3, nil, 0) }
    assert_raises(RangeError) { Outbox.resumed(3, 65_536, 0) }
    assert_raises(RangeError) { Outbox.resumed(0, 0, 0) }
    assert_raises(ArgumentError) { Outbox.resumed(3, 0) }
    assert_equal 2**48 - 1, Outbox.resumed(3, 0, 2**48 - 1).issued
  end

  def test_scope_is_a_strict_integer
    [1.0, nil, '1', 1r, Object.new].each { |bad| assert_raises(TypeError) { Outbox.with_scope(3, bad) } }
    [-1, 65_536, 2**64].each { |bad| assert_raises(RangeError) { Outbox.with_scope(3, bad) } }
    assert_raises(TypeError) { Outbox.with_scope(nil, 1) }
    assert_raises(RangeError) { Outbox.with_scope(0, 1) }
    assert_raises(ArgumentError) { Outbox.with_scope(3) }
  end
end
