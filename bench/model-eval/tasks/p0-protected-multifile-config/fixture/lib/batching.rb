# frozen_string_literal: true

module Batching
  def self.chunks(items, limit:)
    raise ArgumentError, 'batch limit must be a positive Integer' unless limit.is_a?(Integer) && limit.positive?

    batches = []
    cursor = 0
    while cursor + limit <= items.length
      batches << items.slice(cursor, limit)
      cursor += limit
    end
    batches
  end
end
