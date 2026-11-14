# frozen_string_literal: true

require 'minitest/autorun'
require 'outbox_machines'

# The values are sealed, and sealing stops accidents, not reflection: the
# extension is the authority, and every value it reads back must be exactly
# its class and valid by the core, whatever built it.
class SealTest < Minitest::Test
  include OutboxMachines

  VALUES = [Claimed, Retry, Failed, AlreadyFailed, Expired, Sweep, Entry].freeze

  # A named subclass raises too, but Ruby binds the constant before
  # `inherited` runs: the class leaks, and can be reopened to skip the check.
  def leak(value)
    name = :"Leak#{value.name.split('::').last}"
    return self.class.const_get(name) if self.class.const_defined?(name, false)

    assert_raises(TypeError) { self.class.class_eval("class #{name} < #{value}; end", __FILE__, __LINE__) }
    self.class.const_get(name).tap do |leaked|
      leaked.class_eval { def initialize(**fields) = Data.instance_method(:initialize).bind_call(self, **fields) }
    end
  end

  def test_values_cannot_be_subclassed_or_allocated
    VALUES.each do |value|
      assert_raises(TypeError) { Class.new(value) }
      assert_operator leak(value), :<, value
      assert_raises(NoMethodError) { value.allocate }
    end
  end

  # A forged value never reaches the core: not a leaked subclass, not one
  # whose fields `Marshal.load` rewrote, not an Array subclass whose `all?`
  # says yes.
  def test_forged_values_are_refused_where_the_extension_reads_them
    retry_at = leak(Retry).new(at: -1)
    expired = leak(Expired).new(id: 1, attempt: 0, disposition: :dead)
    [Failed, AlreadyFailed].each { |failure| assert_raises(TypeError) { failure.new(retry_at) } }
    assert_raises(TypeError) { Expired.new(1, 1, retry_at) }
    assert_raises(TypeError) { Sweep.new([expired], 3, :partial) }

    negative = Marshal.load(Marshal.dump(Retry.new(5)).sub("i\x0A", "i\xFA".b))
    assert_equal(-1, negative.at)
    assert_raises(RangeError) { Failed.new(negative) }
    assert_raises(RangeError) { Expired.new(1, 1, negative) }
    unissued = Marshal.load(Marshal.dump(Expired.new(7, 1, :dead)).sub("i\x06", "i\x00"))
    assert_equal 0, unissued.attempt
    assert_raises(RangeError) { Sweep.new([unissued], 3, :partial) }

    lying = Class.new(Array) { def all?(*) = true }
    assert_raises(TypeError) { Sweep.new(lying[1, 'x'].freeze, 5, :partial) }
    assert_raises(TypeError) { Sweep.new([Expired.new(7, 1, :dead), 1], 5, :partial) }
  end

  # The classes the extension resolved at init are pinned: after a
  # compaction that moves what it can, every path that builds or reads a
  # value still finds them.
  def test_resolved_classes_survive_compaction
    GC.verify_compaction_references(expand_heap: true, toward: :empty) if GC.respond_to?(:verify_compaction_references)
    GC.compact if GC.respond_to?(:compact)
    box = Outbox.new(3)
    box.enqueue(1, 0)
    box.enqueue(2, 0)
    claimed = box.claim(0, 10)
    assert_instance_of Claimed, claimed
    assert_equal Failed.new(Retry.new(5)), box.fail(1, claimed.attempt, 1, 4)
    assert_instance_of Claimed, box.claim(5, 10)
    assert_equal :complete, box.expire_claims(50, 64).pass
    assert_instance_of Entry, box.get(1)
    assert_raises(TypeError) { Sweep.new([1], 1, :partial) }
    reaped = Expired.new(1, 1, Retry.new(3))
    assert_equal [reaped], Sweep.new([reaped], 1, :partial).expired
  end
end
