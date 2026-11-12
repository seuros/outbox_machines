# frozen_string_literal: true

require_relative 'outbox_machines/version'

module OutboxMachines
  # A live claim: the record id and the attempt generation that must accompany
  # `confirm` and `fail`.
  Attempt = Data.define(:id, :generation)
end

# Native is required: no algorithm fallback. It builds Attempt values, so the
# Data class above must exist first.
require 'outbox_machines_native/outbox_machines_native'
