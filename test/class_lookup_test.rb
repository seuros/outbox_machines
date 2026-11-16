# frozen_string_literal: true

require 'minitest/autorun'
require 'outbox_machines'
require 'open3'
require 'rbconfig'

# The class of the outbox the extension wraps is fixed when it loads. Nothing
# a caller does later, redefining the constant or `Object.const_get`, reaches
# an outbox being built. Each case runs in a fresh process: the class is fixed
# once per process, at load.
class ClassLookupTest < Minitest::Test
  LIB = File.expand_path('../lib', __dir__)

  PRELUDE = <<~RUBY
    require 'outbox_machines'
    O = OutboxMachines
  RUBY

  # Runs `body` after PRELUDE in a new Ruby; answers its last line of output.
  def isolated(body)
    out, err, status = Open3.capture3(RbConfig.ruby, '-I', LIB, '-e', PRELUDE + body)
    assert status.success?, "exited #{status.exitstatus}: #{err}"
    out.lines.last&.chomp
  end

  # Bug: an Outbox constant redefined before the first outbox made it an
  # instance of the new class, without any of the gem's methods.
  def test_a_redefined_outbox_constant_does_not_change_the_outbox_class
    assert_equal 'true true true 1 5 2', isolated(<<~RUBY)
      original = O::Outbox
      O.send(:remove_const, :Outbox)
      O.const_set(:Outbox, Class.new)
      boxes = [original.new(3), original.with_scope(3, 5), original.resumed(3, 5, 2)]
      fresh, scoped, resumed = boxes
      fresh.enqueue(1, 0)
      puts [*boxes.map { |b| b.instance_of?(original) }, fresh.size, scoped.scope, resumed.issued].join(' ')
    RUBY
  end

  # Bug: building the first outbox ran `Object.const_get`, a caller's code in
  # the middle of a native call.
  def test_building_an_outbox_runs_no_const_get
    assert_equal '0 false Outbox Outbox Outbox', isolated(<<~RUBY)
      hits = 0
      log = []
      Object.define_singleton_method(:const_get) do |*args|
        hits += 1
        log.freeze
        super(*args)
      end
      boxes = [O::Outbox.new(3), O::Outbox.with_scope(3, 5), O::Outbox.resumed(3, 5, 2)]
      puts [hits, log.frozen?, *boxes.map { |b| b.class.name.split('::').last }].join(' ')
    RUBY
  end

  # Bug: a redefined `Object.const_get` that raised hit an `unwrap` in the
  # extension: an uncatchable fatal, and the process exited.
  def test_a_raising_const_get_does_not_kill_the_process
    assert_equal '1 5 2', isolated(<<~RUBY)
      Object.define_singleton_method(:const_get) { |*| raise IOError, 'lookup ran mid-call' }
      fresh = O::Outbox.new(3)
      fresh.enqueue(1, 0)
      puts [fresh.size, O::Outbox.with_scope(3, 5).scope, O::Outbox.resumed(3, 5, 2).issued].join(' ')
    RUBY
  end
end
