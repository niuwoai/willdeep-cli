# frozen_string_literal: true

module Batching
  def self.chunks(items, limit:)
    raise ArgumentError, 'batch limit must be a positive Integer' unless limit.is_a?(Integer) && limit.positive?

    items.each_slice(limit).to_a
  end
end
