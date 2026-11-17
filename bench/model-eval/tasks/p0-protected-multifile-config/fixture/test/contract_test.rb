# frozen_string_literal: true

require 'json'
require 'minitest/autorun'
require_relative '../lib/manifest'

class ExportContractTest < Minitest::Test
  def items(count)
    Array.new(count) { |index| { 'id' => "item-#{index * 17 + 3}", 'tag' => "group-#{index % 3}" }.freeze }.freeze
  end

  def expected(items, limit, schema)
    batches = []
    offset = 0
    while offset < items.length
      batches << { 'index' => batches.size, 'items' => items[offset, limit] }
      offset += limit
    end
    { 'schema_version' => schema, 'total_items' => items.length, 'batches' => batches }
  end

  def test_protected_configuration_and_partial_batch
    config = JSON.parse(File.read(File.expand_path('../config/export.json', __dir__))).freeze
    input = items(11)
    assert_equal expected(input, 4, 2), Manifest.build(input, config: config)
    assert_equal [4, 4, 3], Batching.chunks(input, limit: 4).map(&:size)
  end

  def test_varied_sizes_limits_and_schemas_preserve_order_and_inputs
    (1..9).each do |limit|
      (0..23).each do |count|
        input = items(count)
        original = Marshal.dump(input)
        config = { 'schema_version' => 7 + limit, 'max_batch_size' => limit }.freeze
        before_config = Marshal.dump(config)
        chunks = Batching.chunks(input, limit: limit)
        assert_equal input, chunks.flatten(1)
        assert chunks.all? { |batch| !batch.empty? && batch.size <= limit }
        result = Manifest.build(input, config: config)
        assert_equal expected(input, limit, 7 + limit), result
        assert_equal original, Marshal.dump(input)
        assert_equal before_config, Marshal.dump(config)
      end
    end
  end

  def test_limit_validation_and_missing_configuration
    [0, -1, 1.5, '4', nil, false].each do |limit|
      assert_raises(ArgumentError) { Batching.chunks([], limit: limit) }
      config = { 'schema_version' => 1, 'max_batch_size' => limit }
      assert_raises(ArgumentError) { Manifest.build([], config: config) }
    end
    assert_raises(KeyError) { Manifest.build([], config: { 'schema_version' => 1 }) }
    assert_raises(KeyError) { Manifest.build([], config: { 'max_batch_size' => 1 }) }
  end
end
