# frozen_string_literal: true

require 'minitest/autorun'
require 'outbox_machines'

# The binding never loses work. `claim`, `fail` and `expire_claims` answer
# Ruby objects, so the core decides on a copy, the answer is built, and the
# copy is kept only if the outbox is still unfrozen and unchanged. A `new`
# that raises, calls back in, or freezes the outbox applies nothing.
class WorkTest < Minitest::Test
  include OutboxMachines

  # Each mutating call that builds values: the outbox it runs on, the call,
  # and every class whose `new` it runs.
  CALLS = {
    claim: [->(box) { box.enqueue(1, 0) }, ->(box) { box.claim(5, 10) }, [Claimed]],
    fail: [
      ->(box) { box.enqueue(1, 0) && box.claim(0, 10) },
      ->(box) { box.fail(1, box.get(1).attempt, 5, 7) },
      [Failed, Retry]
    ],
    expire_claims: [
      ->(box) { box.enqueue(1, 0) && box.claim(0, 10) },
      ->(box) { box.expire_claims(50, 64) },
      [Sweep, Expired, Retry]
    ]
  }.freeze

  # Runs the block with `klass.new` replaced by `body`, the way reopening the
  # class could redefine it; the real `new` is `klass.__real_new`.
  def replacing_new(klass, body)
    klass.singleton_class.alias_method(:__real_new, :new)
    klass.define_singleton_method(:new, &body)
    yield
  ensure
    klass.singleton_class.alias_method(:new, :__real_new)
    klass.singleton_class.remove_method(:__real_new)
  end

  def state(box) = [box.issued, box.size, box.get(1), box.get(99), box.count(:pending)]

  # The outbox and a twin given the same history.
  def pair(setup) = Array.new(2) { Outbox.new(3).tap(&setup) }

  def test_a_new_that_raises_applies_nothing
    CALLS.each do |name, (setup, call, built)|
      built.each do |klass|
        box, twin = pair(setup)
        replacing_new(klass, ->(*) { raise IOError }) { assert_raises(IOError, name.to_s) { call.(box) } }
        assert_equal state(twin), state(box), name
        # Not even the clock or the sweep cursor moved.
        assert_equal call.(twin), call.(box), name
      end
    end
  end

  def test_a_new_that_calls_back_in_applies_nothing
    CALLS.each do |name, (setup, call, built)|
      built.each do |klass|
        box, twin = pair(setup)
        reenter = lambda do |*args|
          box.enqueue(99, 0)
          klass.__real_new(*args)
        end
        replacing_new(klass, reenter) { assert_raises(RuntimeError, name.to_s) { call.(box) } }
        twin.enqueue(99, 0)
        assert_equal state(twin), state(box), name
        assert_equal call.(twin), call.(box), name
      end
    end
  end

  def test_a_new_that_freezes_the_outbox_applies_nothing
    CALLS.each do |name, (setup, call, built)|
      built.each do |klass|
        box, twin = pair(setup)
        freeze = lambda do |*args|
          box.freeze
          klass.__real_new(*args)
        end
        replacing_new(klass, freeze) { assert_raises(FrozenError, name.to_s) { call.(box) } }
        assert_equal state(twin), state(box), name
      end
    end
  end

  def test_every_mutating_call_refuses_a_frozen_outbox_up_front
    box = Outbox.new(3).tap { _1.enqueue(1, 0) }.freeze
    before = state(box)
    [
      -> { box.enqueue(2, 0) }, -> { box.claim(0, 10) }, -> { box.confirm(1, 1) },
      -> { box.fail(1, 1, 0, 1) }, -> { box.expire_claims(0, 64) }, -> { box.forget(1) }
    ].each { |call| assert_raises(FrozenError) { call.() } }
    assert_equal before, state(box)
  end
end
