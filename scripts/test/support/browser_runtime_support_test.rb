#!/usr/bin/env ruby
# frozen_string_literal: true

# Offline helper checks only: no listeners, binary, Node, Chrome or user HOME.
require 'minitest/autorun'
require 'stringio'
require 'tmpdir'
require_relative 'browser_runtime_fixture'
require_relative '../browser_runtime_entrypoints_test'

class BrowserRuntimeSupportTest < Minitest::Test
  def fixture(id: 'offline', scenario: 'approval_cancel')
    value = BrowserRuntimeFixture.allocate
    value.instance_variable_set(:@mutex, Mutex.new)
    value.instance_variable_set(:@errors, [])
    value.instance_variable_set(:@origin, 'http://127.0.0.1:12345')
    value.instance_variable_set(:@cases, {id => {id: id, scenario: scenario, step: 0, gate: Queue.new,
      gate_entered: false, calls: [], receipts: [], tool_messages: [], input_ref: 'fresh-host-ref'}})
    value
  end

  def test_sse_preserves_fragmented_utf8_and_frame_boundaries
    events = []
    decoder = BrowserRuntime::SseDecoder.new { |event| events << event }
    bytes = ": keepalive\r\n\r\ndata: {\"type\":\"submitted\",\"text\":\"你好\"}\r\n\r\ndata: {\"type\":\"completed\"}\n\n".b
    bytes.bytes.each { |byte| decoder.feed(byte.chr.b) }
    assert_equal [{'type' => 'submitted', 'text' => '你好'}, {'type' => 'completed'}], events
  end

  def test_malformed_sse_is_failure_not_a_terminal_receipt
    decoder = BrowserRuntime::SseDecoder.new { |_event| flunk 'malformed JSON must not emit a receipt' }
    assert_raises(JSON::ParserError) { decoder.feed("data: {broken}\n\n") }
  end

  def test_await_requires_an_observation_and_reports_timeout
    error = assert_raises(Timeout::Error) { BrowserRuntime.await('missing gate', seconds: 0.002) { false } }
    assert_includes error.message, 'missing gate'
    assert_equal 'observed', BrowserRuntime.await('already signalled') { 'observed' }
  end

  def test_child_tree_keeps_unrelated_and_reparented_groups_out
    rows = [
      {pid: 20, ppid: 10, pgid: 20, state: 'S'},
      {pid: 21, ppid: 20, pgid: 20, state: 'S'},
      {pid: 90, ppid: 1, pgid: 90, state: 'S'},
      {pid: 22, ppid: 21, pgid: 20, state: 'Z'}
    ]
    owned = BrowserRuntime.descendants(10, rows)
    assert_equal [20, 21, 22], owned.map { |row| row[:pid] }
    assert_equal [20, 21], BrowserRuntime.running(owned).map { |row| row[:pid] }
  end

  def test_clean_environment_stays_in_private_directories
    Dir.mktmpdir('browser-helper-env-') do |root|
      env = BrowserRuntime.environment(root, '/fixed/node')
      %w[HOME USERPROFILE WILLDEEP_HOME XDG_CONFIG_HOME XDG_DATA_HOME XDG_CACHE_HOME TMPDIR TMP TEMP].each do |key|
        assert env.fetch(key).start_with?(root + '/'), key
      end
      assert_equal '/fixed:/usr/bin:/bin:/usr/sbin:/sbin', env.fetch('PATH')
      assert_equal '127.0.0.1 40000 127.0.0.1 22', env.fetch('SSH_CONNECTION')
      refute env.key?('NODE_OPTIONS')
      refute env.key?('OPENAI_API_KEY')
      refute env.key?('ANTHROPIC_API_KEY')
    end
  end

  def test_fill_uses_only_latest_host_snapshot_reference
    value = fixture
    receipt = {'action' => 'snapshot', 'nodes' => [{'id' => 'name', 'ref' => 'fresh-ref'}]}
    call = value.send(:success_step, 'offline', 3, receipt)
    assert_equal 'fresh-ref', call.fetch(:ref)
    assert_raises(RuntimeError) { value.send(:success_step, 'offline', 3, {'action' => 'open'}) }
    assert_raises(RuntimeError) { value.send(:success_step, 'offline', 3, {'action' => 'snapshot', 'nodes' => []}) }
  end

  def test_failed_business_assertion_does_not_take_screenshot
    value = fixture
    error = assert_raises(RuntimeError) { value.send(:success_step, 'offline', 8, {'passed' => false}) }
    assert_includes error.message, 'business assertion'
  end

  def test_approval_gate_waits_for_explicit_release
    value = fixture
    current = value.instance_variable_get(:@cases).fetch('offline')
    worker = Thread.new { value.send(:cancel_step, 'offline', 4, {'artifact' => {'path' => '/synthetic.png'}}, current) }
    BrowserRuntime.await('offline gate entered', seconds: 1) { value.state('offline')['gate_entered'] }
    assert worker.alive?, 'gate returned before an explicit handshake release'
    current.fetch(:gate).push(:release)
    assert worker.join(1), 'gate failed to react to explicit release'
    assert_equal({action: 'fill', ref: 'fresh-host-ref', text: 'Must not commit offline'}, worker.value)
  ensure
    worker&.kill if worker&.alive?
  end

  def test_cancellation_must_not_issue_another_provider_step
    value = fixture
    error = assert_raises(RuntimeError) { value.send(:cancel_step, 'offline', 5, {}, {}) }
    assert_includes error.message, 'followed by Provider step'
  end

  def test_setup_failure_writes_nine_failures_and_both_report_formats
    Dir.mktmpdir('browser-helper-report-') do |root|
      suite = BrowserRuntimeEntrypoints.new(fixture: File.join(root, 'missing-manifest.json'),
        binary: '/synthetic/missing-binary', report_dir: root)
      capture_io { refute suite.run }
      report = JSON.parse(File.read(File.join(root, 'browser-runtime-entrypoints.json')))
      assert_equal 9, report.fetch('cases').length
      assert report.fetch('cases').all? { |row| row.fetch('status') == 'failed' }
      assert_equal false, report.fetch('passed')
      assert File.read(File.join(root, 'browser-runtime-entrypoints.md')).include?('九例必须全部通过')
    end
  end
end
