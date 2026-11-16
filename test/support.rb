# frozen_string_literal: true

# Swaps methods on the gem's classes for the duration of a block.
module Redefining
  private

  # Runs the block with each method of `klass` named in `bodies` replaced by
  # its body, then restores the originals.
  def redefining(klass, **bodies)
    originals = bodies.to_h { |name, _| [name, klass.instance_method(name)] }
    bodies.each { |name, body| replace_method(klass, name, body) }
    yield
  ensure
    originals&.each { |name, method| replace_method(klass, name, (method if method.owner == klass)) }
  end

  # Defines `name` on `klass` as `body` (removes it when `body` is nil),
  # removing the method `klass` itself holds first. Ruby warns about the
  # redefinition, deliberate here, so warnings are off for the swap alone.
  def replace_method(klass, name, body)
    verbose = $VERBOSE
    $VERBOSE = nil
    klass.remove_method(name) if klass.method_defined?(name, false) || klass.private_method_defined?(name, false)
    klass.define_method(name, body) if body
  ensure
    $VERBOSE = verbose
  end
end
