# frozen_string_literal: true
require_relative 'lib/outbox_machines/version'

Gem::Specification.new do |spec|
  spec.name = 'outbox_machines'
  spec.version = OutboxMachines::VERSION
  spec.authors = ['Abdelkader Boudih']
  spec.email = ['oss@seuros.com']
  spec.summary = 'Delivery ledger for an outbox: claims, leases and attempt identities for Ruby'
  spec.description = 'Tracks every message owed to the outside world as pending, claimed by one ' \
                     'attempt, confirmed or dead. Each claim carries an attempt identity issued once ' \
                     'and never again, so a late worker cannot settle an attempt handed to someone ' \
                     'else, and a lease that runs out is reported as an unknown outcome, not a ' \
                     'failure. Bound to Ruby from the no_std outbox-machines crate: the delivery ' \
                     'policy only, not a transactional outbox, a queue or a retry engine.'
  spec.homepage = 'https://github.com/seuros/outbox_machines'
  spec.license = 'MIT'
  spec.required_ruby_version = '>= 4.0.0'
  spec.metadata['source_code_uri'] = spec.homepage
  spec.metadata['rubygems_mfa_required'] = 'true'
  spec.metadata['cargo_crate_name'] = 'outbox_machines_native'
  spec.metadata['cargo_manifest_path'] = 'ext/outbox_machines_native/ffi/Cargo.toml'
  spec.files = Dir.glob(%w[
    lib/**/*.rb sig/**/*.rbs ext/**/*.{rs,rb,toml,md} Cargo.toml
    Cargo.lock README.md CHANGELOG.md LICENSE.txt
  ]).select { |p| File.file?(p) }
    .reject { |p| p.end_with?('/tests.rs') || p.include?('/tests/') }
  spec.require_paths = ['lib']
  spec.extensions = ['ext/outbox_machines_native/extconf.rb']
  spec.add_dependency 'rb_sys', '~> 0.9', '>= 0.9.130'
end
