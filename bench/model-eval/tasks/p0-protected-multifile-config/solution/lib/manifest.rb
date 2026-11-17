# frozen_string_literal: true

require_relative 'batching'

module Manifest
  def self.build(items, config:)
    batches = Batching.chunks(items, limit: config.fetch('max_batch_size'))
    {
      'schema_version' => config.fetch('schema_version'),
      'total_items' => items.length,
      'batches' => batches.each_with_index.map { |batch, index| { 'index' => index, 'items' => batch } }
    }
  end
end
