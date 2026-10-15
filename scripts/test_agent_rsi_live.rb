#!/usr/bin/env ruby
# frozen_string_literal: true
require 'minitest/autorun'
require 'tmpdir'
require 'minitest/mock'
require_relative 'lib/agent_rsi_live'
require_relative 'lib/agent_rsi_live_host'

module AgentRsiFreeReporting
  class << self
    attr_accessor :reporter
    def minitest_plugin_init(_options)
      self.reporter = Minitest.reporter
    end
  end
end
Minitest.extensions << AgentRsiFreeReporting

report_index = ARGV.index('--report-directory')
if report_index
  report_directory = File.expand_path(ARGV.fetch(report_index + 1))
  ARGV.slice!(report_index, 2)
  Minitest.after_run do
    summary = AgentRsiFreeReporting.reporter.reporters.find { |reporter| reporter.is_a?(Minitest::SummaryReporter) }
    result = {'tests' => summary.count, 'assertions' => summary.assertions, 'failures' => summary.failures,
              'errors' => summary.errors, 'skipped' => summary.skips, 'paid_model_calls' => 0,
              'passed' => summary.failures.zero? && summary.errors.zero?}
    FileUtils.mkdir_p(report_directory, mode: 0o700)
    AgentRsiLive.write(File.join(report_directory, 'budget-and-grader.json'), result)
    AgentRsiLive.write(File.join(report_directory, 'budget-and-grader.md'),
                      "# 真实实验门禁免费测试\n\n#{result['tests']} 项，#{result['assertions']} 个断言，失败 #{result['failures']}，错误 #{result['errors']}。\n\n真实模型请求：0。\n")
  end
end

class AgentRsiLiveTest < Minitest::Test
  def setup
    @directory = Dir.mktmpdir('rsi-live-free-test-')
    @ledger = AgentRsiLive::Ledger.new(@directory, {
      'proposal_sha256' => AgentRsiLive::PROPOSAL,
      'maximum_bill_microusd_per_million_tokens' => 694_050
    })
    @trial = {'trial_id' => 'one', 'platform' => 'rust-native-worker', 'task_id' => 'original',
              'split' => 'validation', 'arm' => 'baseline', 'repetition' => 1,
              'attempt' => 1, 'resume_of' => nil, 'agent_revision' => AgentRsiLive::REVISIONS['baseline']}
    @body = {'model' => AgentRsiLive::MODEL, 'temperature' => 0, 'stream' => false,
             'max_tokens' => 4000, 'messages' => [{'role' => 'user', 'content' => 'read fixture.txt'}]}
    @ledger.begin_trial(@trial)
  end

  def teardown
    @ledger.close
    FileUtils.remove_entry(@directory)
  end

  def settled(sequence = 1, headers = nil)
    headers ||= {'x-request-id' => sequence == 1 ? 'real-request' : "real-request-#{sequence}", 'x-routed-model' => 'fixed-route'}
    record, = @ledger.authorize_request('one', sequence, @body)
    @ledger.settle(record, {'id' => 'response-real', 'usage' => {'prompt_tokens' => 50, 'completion_tokens' => 20, 'total_tokens' => 70}}, headers)
    record
  end

  def test_pre_dispatch_reservation_is_durable
    row, = @ledger.authorize_request('one', 1, @body)
    persisted = JSON.parse(File.read(@ledger.path)).fetch('requests').first
    assert_equal 'reserved', persisted['status']
    assert_equal row['reserved_microusd'], persisted['reserved_microusd']
    assert_operator persisted['reserved_tokens'], :<=, 16_000
  end

  def test_original_suite_uses_shared_cross_platform_canonical_hash
    suite = JSON.parse(File.read(File.join(AgentRsiLive::Host::ROOT, 'bench/agent-rsi/reviewer-original-24-v1/suite.json')))
    assert_equal AgentRsiLive::SUITE, AgentRsiLive.digest(suite)
    assert_equal '88c434bb710603a44ff0c36180af781159aaab3f99363bb9f4432b27993a44d9', AgentRsiLive.digest(suite['tasks'].first)
  end

  def test_no_parallel_or_replayed_requests
    @ledger.authorize_request('one', 1, @body)
    assert_raises(AgentRsiLive::Denied) { @ledger.authorize_request('one', 2, @body) }
    assert_raises(AgentRsiLive::Denied) { @ledger.begin_trial(@trial.merge('trial_id' => 'two')) }
  end

  def test_no_model_upgrade
    assert_raises(AgentRsiLive::Denied) { @ledger.authorize_request('one', 1, @body.merge('model' => 'other')) }
    assert_empty @ledger.state['requests']
  end

  def test_four_request_limit
    4.times { |index| settled(index + 1) }
    assert_raises(AgentRsiLive::Denied) { @ledger.authorize_request('one', 5, @body) }
  end

  def test_unknown_usage_preserves_exposure_and_stops
    row, = @ledger.authorize_request('one', 1, @body)
    assert_raises(AgentRsiLive::Denied) { @ledger.settle(row, {'id' => 'response'}, {}) }
    assert_equal 'unknown', row['status']
    refute row.key?('accounted_microusd')
    assert_operator row['reserved_microusd'], :>, 0
    assert_raises(AgentRsiLive::Denied) { @ledger.authorize_request('one', 2, @body) }
  end

  def test_route_change_stops_batch
    settled
    assert_raises(AgentRsiLive::Denied) { settled(2, {'x-request-id' => 'second', 'x-routed-model' => 'upgraded'}) }
    assert_equal 'provider route differs from frozen mapping', @ledger.state['paused']
  end

  def test_reused_provider_request_id_stops_reconciliation_ambiguity
    settled
    assert_raises(AgentRsiLive::Denied) { settled(2, {'x-request-id' => 'real-request', 'x-routed-model' => 'fixed-route'}) }
    assert_equal 'provider request id was reused', @ledger.state['paused']
  end

  def test_conservative_input_bound_can_reject_without_dispatch
    large = @body.merge('messages' => [{'role' => 'system', 'content' => 'a' * 16_001}])
    assert_raises(AgentRsiLive::Denied) { @ledger.authorize_request('one', 1, large) }
    assert_empty @ledger.state['requests']
  end

  def test_remaining_budget_reduces_output_before_dispatch
    @ledger.state['requests'] << {'trial_id' => 'one', 'status' => 'settled', 'actual_tokens' => 12_500,
                                  'reserved_tokens' => 12_500, 'accounted_microusd' => 10_000}
    record, admitted = @ledger.authorize_request('one', 2, @body)
    assert_operator admitted['max_tokens'], :<, 4000
    assert_equal 3500, record['reserved_tokens']
  end

  def test_quality_failure_cannot_retry
    @ledger.finish_trial('quality_failure')
    assert_raises(AgentRsiLive::Denied) { @ledger.begin_trial(@trial.merge('trial_id' => 'retry', 'attempt' => 2, 'resume_of' => 'one')) }
  end

  def test_infrastructure_retry_consumed_once
    @ledger.finish_trial('infrastructure_error')
    @ledger.begin_trial(@trial.merge('trial_id' => 'retry', 'attempt' => 2, 'resume_of' => 'one'))
    @ledger.finish_trial('infrastructure_error')
    assert_equal 1, @ledger.state['retries']
    assert_raises(AgentRsiLive::Denied) { @ledger.begin_trial(@trial.merge('trial_id' => 'retry-two', 'attempt' => 2, 'resume_of' => 'one')) }
  end

  def test_dollar_cap_counts_unknown_reservations
    @ledger.state['requests'] << {'trial_id' => 'other', 'status' => 'unknown', 'reserved_tokens' => 1,
                                  'reserved_microusd' => 4_999_999}
    assert_raises(AgentRsiLive::Denied) { @ledger.authorize_request('one', 1, @body) }
  end

  def test_holdout_accesses_never_refunded
    @ledger.finish_trial('passed')
    @ledger.state['holdout_accesses'] = 90
    assert_raises(AgentRsiLive::Denied) { @ledger.begin_trial(@trial.merge('trial_id' => 'holdout', 'task_id' => 'h', 'split' => 'holdout')) }
    assert_equal 90, @ledger.state['holdout_accesses']
  end

  def test_no_auto_resume_or_overwrite
    assert_raises(AgentRsiLive::Denied) do
      AgentRsiLive::Ledger.new(@directory, {'proposal_sha256' => AgentRsiLive::PROPOSAL,
                                          'maximum_bill_microusd_per_million_tokens' => 694_050})
    end
  end

  def grade(raw)
    AgentRsiLive::Grader.grade(raw, {'has_defect' => true, 'line' => 3}, read_observed: true, fixture_unchanged: true)
  end

  def test_grader_uses_model_object_inside_normal_report
    result = grade("CONCLUSION\n```json\n{\"has_defect\":true,\"line\":3,\"reason\":\"Zero is wrongly replaced\"}\n```\nEVIDENCE\nfixture.txt:3")
    assert result['passed']
  end

  def test_grader_rejects_multiple_answers_and_wrong_schema
    raw = '{"has_defect":true,"line":3,"reason":"why"}'
    refute grade(raw + raw)['passed']
    refute grade('{"has_defect":"true","line":3,"reason":"why"}')['schema_valid']
    refute grade('{"has_defect":true,"line":3,"reason":"why","passed":true}')['schema_valid']
    refute grade('{"has_defect":false,"has_defect":true,"line":3,"reason":"why"}')['schema_valid']
    refute grade(raw + '{broken}')['schema_valid']
  end

  def test_classification_and_exact_line_are_separate
    result = grade('{"has_defect":true,"line":4,"reason":"dereference crashes"}')
    assert result['classification_correct']
    refute result['localization_correct']
    refute result['passed']
  end

  def test_no_self_report_read_credit
    result = AgentRsiLive::Grader.grade('{"has_defect":true,"line":3,"reason":"I read fixture.txt"}',
                                       {'has_defect' => true, 'line' => 3}, read_observed: false, fixture_unchanged: true)
    refute result['passed']
    refute result['read_observed']
  end

  FakeRequest = Struct.new(:request_method, :path, :body, :headers) do
    def [](key) = headers[key]
  end
  FakeResponse = Struct.new(:status, :body, :headers) do
    def []=(key, value)
      headers[key] = value
    end
  end
  UpstreamResponse = Struct.new(:code, :body, :headers) do
    def [](key) = headers[key]
  end

  def broker_request(capability = 'test-capability')
    FakeRequest.new('POST', '/v1/chat/completions', JSON.generate(@body), {
      'authorization' => 'Bearer ' + capability, 'x-rsi-trial-id' => 'one',
      'x-rsi-proposal-sha256' => AgentRsiLive::PROPOSAL, 'x-rsi-request-sequence' => '1'
    })
  end

  def isolated_broker(result, &inspect_request)
    # Exercise the actual handler with an injected in-memory transport. No
    # socket, DNS, provider key or model API is available in these tests.
    broker = AgentRsiLive::Broker.allocate
    {ledger: @ledger, upstream: URI('https://invalid.example/v1/chat/completions'),
     api_key: 'test-only-not-a-secret', directory: @directory, trial_id: 'one', capability: 'test-capability',
     transport: ->(request) { inspect_request&.call(request); result }}.each do |key, value|
      broker.instance_variable_set('@' + key.to_s, value)
    end
    broker
  end

  def test_broker_stale_capability_does_not_dispatch_or_reserve
    calls = 0
    broker = isolated_broker(nil) { calls += 1 }
    response = FakeResponse.new(nil, nil, {})
    broker.handle(broker_request('expired-capability'), response)
    assert_equal 403, response.status
    assert_equal 0, calls
    assert_empty @ledger.state['requests']
  end

  def test_broker_persists_before_transport_and_returns_real_route
    upstream = UpstreamResponse.new('200', JSON.generate({
      'id' => 'real-response', 'model' => 'opaque-upstream-id',
      'usage' => {'prompt_tokens' => 100, 'completion_tokens' => 20, 'total_tokens' => 120},
      'choices' => [{'message' => {'content' => 'answer'}, 'finish_reason' => 'stop'}]
    }), {'x-request-id' => 'provider-request-id', 'x-routed-model' => 'fixed-route'})
    broker = isolated_broker(upstream) do |request|
      assert_equal 'reserved', JSON.parse(File.read(@ledger.path))['requests'].last['status']
      assert_equal AgentRsiLive::MODEL, JSON.parse(request.body)['model']
    end
    response = FakeResponse.new(nil, nil, {})
    broker.handle(broker_request, response)
    assert_equal 200, response.status
    assert_equal 'fixed-route', response.headers['x-routed-model']
    assert_equal AgentRsiLive::MODEL, response.headers['X-RSI-Approved-Model']
    assert_equal 'opaque-upstream-id', JSON.parse(response.body)['model']
    assert_equal 'settled', @ledger.state['requests'].last['status']
  end

  def test_upstream_failure_keeps_unknown_exposure_and_does_not_retry
    calls = 0
    broker = isolated_broker(UpstreamResponse.new('502', '{}', {})) { calls += 1 }
    response = FakeResponse.new(nil, nil, {})
    broker.handle(broker_request, response)
    assert_equal 403, response.status
    assert_equal 1, calls
    assert_equal 'unknown', @ledger.state['requests'].last['status']
    assert_match(/HTTP 502/, @ledger.state['paused'])
    assert_operator @ledger.state['requests'].last['reserved_microusd'], :>, 0
  end

  def test_editable_approved_json_is_not_a_valid_grant
    AgentRsiLive.write(File.join(@directory, 'manifest.json'), {'approved' => true, 'run_directory' => @directory})
    key = OpenSSL::PKey::RSA.new(2048)
    payload = {'approved' => true}
    signature = key.sign(OpenSSL::Digest::SHA256.new, AgentRsiLive.canonical(payload))
    grant_path = File.join(@directory, 'invalid-grant.json')
    AgentRsiLive.write(grant_path, {'payload' => payload, 'signature_base64' => [signature].pack('m0')})
    assert_raises(AgentRsiLive::Denied) do
      AgentRsiLive::Host.new(@directory, signed_grant: grant_path, public_key: key.public_key.to_pem)
    end
  end

  def test_paired_comparison_clusters_repetitions_and_excludes_incomplete_pairs
    rows = []
    %w[complete incomplete].each do |task|
      %w[baseline candidate].each do |arm|
        1.upto(3) do |repetition|
          next if task == 'incomplete' && arm == 'candidate' && repetition == 3
          rows << {'platform' => 'rust-native-worker', 'task_id' => task, 'arm' => arm, 'repetition' => repetition,
                   'status' => 'quality_failure', 'verification' => {'classification_correct' => arm == 'candidate'}}
        end
      end
    end
    result = AgentRsiLive::Host.allocate.paired_comparison(rows).first
    assert_equal 1, result['independent_task_pairs']
    assert_equal 1.0, result['candidate_minus_baseline_accuracy']
    assert result['repetitions_are_not_independent_tasks']
  end

  def test_signed_experiment_cannot_replay_in_another_directory
    AgentRsiLive.write(File.join(@directory, 'manifest.json'), {'run_directory' => @directory + '-original'})
    error = assert_raises(AgentRsiLive::Denied) do
      AgentRsiLive::Host.new(@directory, signed_grant: 'must-not-be-read', public_key: 'must-not-be-read')
    end
    assert_match(/another directory/, error.message)
  end

  def test_expired_authorization_cannot_dispatch
    @ledger.instance_variable_set(:@expires_at, Time.now - 1)
    assert_raises(AgentRsiLive::Denied) { @ledger.authorize_request('one', 1, @body) }
    assert_empty @ledger.state['requests']
  end

  def test_known_larger_bill_remains_in_financial_exposure
    receipt = settled
    host = AgentRsiLive::Host.allocate
    account = Object.new
    account.define_singleton_method(:billing_receipts) do |_trial|
      [{'request_id' => 'real-request', 'model' => AgentRsiLive::MODEL, 'cost' => '0.1'}]
    end
    host.instance_variable_set(:@account, account)
    host.instance_variable_set(:@ledger, @ledger)
    assert_raises(AgentRsiLive::Denied) { host.reconcile_bills(@trial, [receipt]) }
    persisted = JSON.parse(File.read(@ledger.path))['requests'].first
    assert_equal 100_000, persisted['observed_bill_microusd']
    assert_equal 100_000, AgentRsiLive::Ledger.exposure(persisted)
    refute persisted.key?('verified_bill_microusd')
  end

  def test_negative_bill_is_not_refunded_to_budget
    receipt = settled
    host = AgentRsiLive::Host.allocate
    account = Object.new
    account.define_singleton_method(:billing_receipts) do |_trial|
      [{'request_id' => 'real-request', 'model' => AgentRsiLive::MODEL, 'cost' => '-1'}]
    end
    host.instance_variable_set(:@account, account)
    host.instance_variable_set(:@ledger, @ledger)
    assert_raises(AgentRsiLive::Denied) { host.reconcile_bills(@trial, [receipt]) }
    assert_operator AgentRsiLive::Ledger.exposure(receipt), :>, 0
    refute receipt.key?('verified_bill_microusd')
  end

  def observation_fixture
    task = {'fixture' => 'source', 'prompt' => 'review', 'expected_output' => '{}'}
    receipt = {'receipt_id' => 'root-receipt', 'request_id' => 'provider-id', 'routed_model' => 'frozen-model', 'status' => 'settled'}
    observation = @trial.reject { |key, _| %w[split agent_revision].include?(key) }.merge(
      'schema' => 'willdeep.agent-rsi.native-observation.v1', 'experiment_id' => 'frozen-experiment',
      'proposal_sha256' => AgentRsiLive::PROPOSAL, 'suite_sha256' => AgentRsiLive::SUITE,
      'definition_revision' => @trial['agent_revision'], 'loaded_revision' => @trial['agent_revision'],
      'task_sha256' => AgentRsiLive.digest(task), 'fixture_sha256' => Digest::SHA256.hexdigest('source'),
      'read_observed' => false, 'fixture_unchanged' => true, 'events' => [], 'events_sha256' => AgentRsiLive.digest([]),
      'effective_prompt_sha256' => 'a' * 64, 'context' => {'root_run_id' => SecureRandom.uuid, 'worker_run_id' => SecureRandom.uuid},
      'runtime' => {'binary_sha256' => 'b' * 64}, 'request_receipts' => [receipt])
    host = AgentRsiLive::Host.allocate
    host.instance_variable_set(:@manifest, {'experiment_id' => 'frozen-experiment', 'rust_binary_sha256' => 'b' * 64})
    [host, observation, receipt, task]
  end

  def test_complete_no_tool_call_is_a_known_read_failure_not_missing_evidence
    host, observation, receipt, task = observation_fixture
    host.validate_observation!(@trial, observation, [receipt], task)
    assert_equal false, observation['read_observed']
  end

  def test_wrong_identity_missing_read_proof_and_unmatched_receipts_are_rejected
    host, observation, receipt, task = observation_fixture
    assert_raises(AgentRsiLive::Denied) { host.validate_observation!(@trial, observation.merge('arm' => 'candidate'), [receipt], task) }
    assert_raises(AgentRsiLive::Denied) { host.validate_observation!(@trial, observation.reject { |key, _| key == 'read_observed' }, [receipt], task) }
    assert_raises(AgentRsiLive::Denied) { host.validate_observation!(@trial, observation.merge('request_receipts' => []), [receipt], task) }
  end

  FreeMetadataStop = Class.new(StandardError)

  def test_valid_scope_signature_reaches_only_the_mocked_account_boundary
    parent = Dir.mktmpdir('rsi-approved-manifest-free-', '/private/tmp')
    binary = File.join(parent, 'fixture-binary')
    frozen = File.join(parent, 'fixture-frozen-runner.json')
    mac = File.join(parent, 'mac-source')
    FileUtils.mkdir_p(File.join(mac, 'scripts'))
    AgentRsiLive.write(binary, 'FREE FIXTURE NOT EXECUTABLE')
    AgentRsiLive.write(frozen, {})
    %w[agent_rsi_approved_trial.rb agent_rsi_frozen_runner.rb].each { |name| AgentRsiLive.write(File.join(mac, 'scripts', name), 'FREE FIXTURE') }
    run = File.join(parent, 'run')
    manifest = AgentRsiLive::Host.prepare(run, rust_binary: binary, mac_root: mac, mac_binary: binary,
      mac_frozen: frozen, account_snapshot: {'maximum_bill_microusd_per_million_tokens' => 694_050})
    key = OpenSSL::PKey::RSA.new(2048)
    payload = {'manifest_sha256' => AgentRsiLive.digest(manifest), 'proposal_sha256' => AgentRsiLive::PROPOSAL,
               'model' => AgentRsiLive::MODEL, 'budget_cap_microusd' => 5_000_000,
               'maximum_bill_microusd_per_million_tokens' => 694_050,
               'direct_human_confirmation_source' => 'FREE TEST AUTHORITY; NOT A LIVE GRANT',
               'expires_at' => (Time.now + 60).utc.iso8601}
    signature = key.sign(OpenSSL::Digest::SHA256.new, AgentRsiLive.canonical(payload))
    grant = File.join(parent, 'fixture-grant.json')
    AgentRsiLive.write(grant, {'payload' => payload, 'signature_base64' => [signature].pack('m0')})
    AgentRsiLive::Account.stub(:new, -> { raise FreeMetadataStop, 'no key, metadata or inference is available in this test' }) do
      assert_raises(FreeMetadataStop) { AgentRsiLive::Host.new(run, signed_grant: grant, public_key: key.public_key.to_pem) }
    end
    assert_equal 288, JSON.parse(File.read(File.join(run, 'plan.json'))).length
  ensure
    FileUtils.remove_entry(parent) if parent && File.exist?(parent)
  end
end
