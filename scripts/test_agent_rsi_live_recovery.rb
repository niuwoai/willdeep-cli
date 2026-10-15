#!/usr/bin/env ruby
# frozen_string_literal: true
require 'minitest/autorun'
require 'minitest/mock'
require 'tmpdir'
require_relative 'lib/agent_rsi_live_host'

module RecoveryFreeReporting
  class << self
    attr_accessor :reporter
    def minitest_plugin_init(_options) = self.reporter = Minitest.reporter
  end
end
Minitest.extensions << RecoveryFreeReporting
report_index = ARGV.index('--report-directory')
if report_index
  report_directory = File.expand_path(ARGV.fetch(report_index + 1))
  ARGV.slice!(report_index, 2)
  Minitest.after_run do
    summary = RecoveryFreeReporting.reporter.reporters.find { |reporter| reporter.is_a?(Minitest::SummaryReporter) }
    result = {'tests' => summary.count, 'assertions' => summary.assertions, 'failures' => summary.failures,
              'errors' => summary.errors, 'paid_model_calls' => 0}
    FileUtils.mkdir_p(report_directory, mode: 0o700)
    AgentRsiLive.write(File.join(report_directory, 'known-terminal-recovery.json'), result)
    AgentRsiLive.write(File.join(report_directory, 'known-terminal-recovery.md'),
      "# 已知终态继续门禁免费测试\n\n#{summary.count} 项，#{summary.assertions} 断言；失败 #{summary.failures}，错误 #{summary.errors}；真实模型调用 0。\n")
  end
end

class AgentRsiLiveRecoveryTest < Minitest::Test
  LedgerFixture = Struct.new(:state)

  def setup
    @directory = Dir.mktmpdir('rsi-recovery-free-')
    @trial = {'trial_id' => 'known', 'platform' => 'rust-native-worker', 'task_id' => 'task', 'split' => 'validation'}
    @directory_trial = File.join(@directory, 'known')
    FileUtils.mkdir_p(@directory_trial)
    AgentRsiLive.write(File.join(@directory, 'suite.json'), {'tasks' => [{'task_id' => 'task'}]})
    AgentRsiLive.write(File.join(@directory, 'known-process.json'), {'exit_status' => 0})
    @observation = {'did_truncate' => true, 'usage_complete' => true, 'input_tokens' => 10, 'output_tokens' => 2,
      'request_receipts' => [{'receipt_id' => 'receipt', 'usage' => {'prompt_tokens' => 10, 'completion_tokens' => 2, 'total_tokens' => 12}}]}
    write_observation
    @ledger = LedgerFixture.new({'attempts' => [@trial.merge('status' => 'budget_exhausted')],
                                'requests' => [{'trial_id' => 'known', 'status' => 'settled', 'verified_bill_microusd' => 776,
                                  'receipt_id' => 'receipt', 'input_tokens' => 10, 'output_tokens' => 2, 'actual_tokens' => 12}]})
    @host = AgentRsiLive::Host.allocate
    @host.instance_variable_set(:@directory, @directory)
    @host.instance_variable_set(:@ledger, @ledger)
    @host.instance_variable_set(:@manifest, {'protocol_revision' => 'v2-known-terminal', 'suite_path' => File.join(@directory, 'suite.json')})
  end

  def teardown
    FileUtils.remove_entry(@directory)
  end

  def write_observation
    AgentRsiLive.write(File.join(@directory_trial, 'observation.json'), @observation)
  end

  def terminal
    @host.stub(:validate_observation!, nil) { @host.prior_terminal!(@trial) }
  end

  def test_v2_is_explicit_and_v1_remains_default
    assert @host.recovery_v2?
    @host.instance_variable_set(:@manifest, {})
    refute @host.recovery_v2?
  end

  def test_known_budget_terminal_can_be_skipped_without_changing_it
    before = Marshal.dump(@ledger.state)
    assert terminal
    assert_equal before, Marshal.dump(@ledger.state)
  end

  def test_budget_with_unknown_bill_is_not_skippable
    @ledger.state['requests'].first.delete('verified_bill_microusd')
    assert_raises(AgentRsiLive::Denied) { terminal }
  end

  def test_budget_with_unknown_usage_is_not_skippable
    @observation['usage_complete'] = false
    write_observation
    assert_raises(AgentRsiLive::Denied) { terminal }
  end

  def test_frozen_mac_schema_derives_complete_usage_without_flag
    @observation.delete('usage_complete')
    write_observation
    assert terminal
  end

  def test_absent_flag_does_not_default_missing_usage_to_complete
    @observation.delete('usage_complete')
    @observation['request_receipts'].first.delete('usage')
    write_observation
    assert_raises(AgentRsiLive::Denied) { terminal }
  end

  def test_absent_flag_still_requires_equal_native_and_broker_totals
    @observation.delete('usage_complete')
    @observation['input_tokens'] = 11
    write_observation
    assert_raises(AgentRsiLive::Denied) { terminal }
  end

  def test_flag_true_cannot_hide_changed_receipt_usage
    @observation['request_receipts'].first['usage']['total_tokens'] = 13
    write_observation
    assert_raises(AgentRsiLive::Denied) { terminal }
  end

  def mac_round_limit_fixture
    trial = @trial.merge('platform' => 'macos-native-worker')
    observation = {'did_truncate' => false, 'raw_model_content' => nil,
      'runtime' => {'adapter' => 'macos-native-worker.v1'},
      'worker_report' => '[Subagent stopped after 4 tool rounds without a final report. Last progress shown below.]'}
    response = {'choices' => [{'finish_reason' => 'tool_calls', 'message' => {'content' => '', 'tool_calls' => [{'id' => 'call'}]}}]}
    receipts = Array.new(4) { |i| {'receipt_id' => i.to_s} }
    receipts.last['response_sha256'] = AgentRsiLive.digest(response)
    @ledger.state['requests'] = receipts
    AgentRsiLive.write(File.join(@directory, 'response-0004.json'), {'body' => JSON.generate(response)})
    [trial, observation, receipts]
  end

  def test_frozen_mac_round_limit_has_known_budget_terminal
    trial, observation, receipts = mac_round_limit_fixture
    assert @host.budget_terminal?(trial, observation, receipts)
  end

  def test_mac_round_limit_requires_all_four_requests
    trial, observation, receipts = mac_round_limit_fixture
    refute @host.budget_terminal?(trial, observation, receipts.first(3))
  end

  def test_mac_round_limit_requires_native_stop_marker
    trial, observation, receipts = mac_round_limit_fixture
    observation['worker_report'] = 'no final output'
    refute @host.budget_terminal?(trial, observation, receipts)
  end

  def test_mac_round_limit_never_reclassifies_final_answer
    trial, observation, receipts = mac_round_limit_fixture
    observation['raw_model_content'] = '{"has_defect":false,"line":null,"reason":"ok"}'
    refute @host.budget_terminal?(trial, observation, receipts)
  end

  def test_mac_round_limit_provider_evidence_must_match_receipt
    trial, observation, receipts = mac_round_limit_fixture
    receipts.last['response_sha256'] = 'changed'
    assert_raises(AgentRsiLive::Denied) { @host.budget_terminal?(trial, observation, receipts) }
  end

  def local_denial_fixture
    root = @ledger.state['requests'].first
    root.merge!('request_id' => 'physical', 'routed_model' => 'route', 'sequence' => 1)
    response = {'choices' => [{'finish_reason' => 'length', 'message' => {'content' => 'partial'}}]}
    root['response_sha256'] = AgentRsiLive.digest(response)
    AgentRsiLive.write(File.join(@directory, 'response-0001.json'), {'http_status' => 200, 'body' => JSON.generate(response)})
    physical = @observation['request_receipts'].first.merge('request_id' => 'physical', 'routed_model' => 'route',
      'sequence' => 1, 'status' => 200, 'transport_valid' => true, 'completion_truncated' => true, 'finish_reason' => 'length')
    denied = {'sequence' => 2, 'status' => 403, 'transport_valid' => false,
      'receipt_id' => nil, 'request_id' => nil, 'response_id' => nil, 'routed_model' => nil, 'usage' => nil}
    observation = @observation.merge('request_receipts' => [physical, denied], 'usage_complete' => false,
      'input_tokens' => nil, 'output_tokens' => nil, 'total_tokens' => nil, 'raw_model_content' => 'partial',
      'worker_status' => 'infrastructure_error', 'runtime' => {'adapter' => 'rust-worker-native-broker-v1'})
    [observation, @ledger.state['requests']]
  end

  def test_local_denial_has_separate_known_physical_usage_view
    observation, receipts = local_denial_fixture
    before = Marshal.dump(observation)
    original_file = File.binread(File.join(@directory_trial, 'observation.json'))
    view = @host.budget_validation_view(@trial, observation, receipts)
    assert @host.complete_usage?(view, receipts)
    assert_equal 12, view['total_tokens']
    assert_equal before, Marshal.dump(observation)
    assert_equal original_file, File.binread(File.join(@directory_trial, 'observation.json'))
    projection = JSON.parse(File.read(File.join(@directory_trial, 'budget-usage-projection.json')))
    assert_equal 0, projection['local_denial_model_calls']
  end

  def test_billing_gate_withheld_paid_response_cannot_be_projected
    observation, receipts = local_denial_fixture
    receipts << receipts.first.merge('receipt_id' => 'withheld', 'sequence' => 2)
    assert_same observation, @host.budget_validation_view(@trial, observation, receipts)
  end

  def test_unknown_physical_usage_cannot_be_projected
    observation, receipts = local_denial_fixture
    receipts.first['status'] = 'unknown'
    assert_same observation, @host.budget_validation_view(@trial, observation, receipts)
  end

  def test_denial_with_upstream_identity_cannot_be_projected
    observation, receipts = local_denial_fixture
    observation['request_receipts'].last['request_id'] = 'possibly-billed'
    assert_same observation, @host.budget_validation_view(@trial, observation, receipts)
  end

  def test_denial_usage_not_explicitly_null_cannot_be_projected
    observation, receipts = local_denial_fixture
    observation['request_receipts'].last.delete('usage')
    assert_same observation, @host.budget_validation_view(@trial, observation, receipts)
  end

  def test_without_delivered_truncation_denial_is_not_budget_projection
    observation, receipts = local_denial_fixture
    observation['did_truncate'] = false
    assert_same observation, @host.budget_validation_view(@trial, observation, receipts)
  end

  def test_projection_must_bind_provider_original_content
    observation, receipts = local_denial_fixture
    observation['raw_model_content'] = 'changed'
    assert_raises(AgentRsiLive::Denied) { @host.budget_validation_view(@trial, observation, receipts) }
  end

  def test_projection_cannot_hide_missing_successful_identity
    observation, receipts = local_denial_fixture
    observation['request_receipts'].first['receipt_id'] = nil
    assert_same observation, @host.budget_validation_view(@trial, observation, receipts)
  end

  def test_projection_receipt_cannot_be_silently_changed
    observation, receipts = local_denial_fixture
    @host.budget_validation_view(@trial, observation, receipts)
    AgentRsiLive.write(File.join(@directory_trial, 'budget-usage-projection.json'), {'changed' => true})
    assert_raises(AgentRsiLive::Denied) { @host.budget_validation_view(@trial, observation, receipts) }
  end

  def reservation_denial_fixture
    observation, receipts = local_denial_fixture
    body = {'max_tokens' => 4000, 'messages' => [{'role' => 'user', 'content' => 'a' * 1200}],
      'model' => AgentRsiLive::MODEL, 'stream' => false, 'temperature' => 0, 'tool_choice' => 'auto', 'tools' => []}
    wire = JSON.generate(body)
    AgentRsiLive.write(File.join(@directory, 'request-0001.json'), body)
    receipts.first.merge!('input_tokens' => 1450, 'output_tokens' => 500, 'actual_tokens' => 1950,
      'requested_body_sha256' => AgentRsiLive.digest(body), 'reserved_input_tokens' => wire.bytesize + 947)
    physical = observation['request_receipts'].first
    physical.merge!('request_sha256' => Digest::SHA256.hexdigest(wire), 'request_utf8_bytes' => wire.bytesize,
      'usage' => {'prompt_tokens' => 1450, 'completion_tokens' => 500, 'total_tokens' => 1950},
      'completion_truncated' => false, 'finish_reason' => 'tool_calls')
    response = {'choices' => [{'finish_reason' => 'tool_calls', 'message' => {'content' => '', 'tool_calls' => [{'id' => 'call'}]}}]}
    receipts.first['response_sha256'] = AgentRsiLive.digest(response)
    AgentRsiLive.write(File.join(@directory, 'response-0001.json'), {'http_status' => 200, 'body' => JSON.generate(response)})
    observation.merge!('did_truncate' => false, 'raw_model_content' => nil)
    observation['request_receipts'].last.merge!('request_utf8_bytes' => 15000, 'request_sha256' => 'b' * 64)
    [observation, receipts]
  end

  def test_input_reservation_denial_is_known_budget_without_truncation
    observation, receipts = reservation_denial_fixture
    view = @host.budget_validation_view(@trial, observation, receipts)
    assert @host.complete_usage?(view, receipts)
    assert @host.budget_terminal?(@trial, observation, receipts)
    assert_equal false, observation['usage_complete']
    projection = JSON.parse(File.read(File.join(@directory_trial, 'budget-usage-projection.json')))
    proof = projection['input_reservation_proof']
    assert_equal 947, proof['native_fixed_envelope_delta']
    assert_operator proof['minimum_trial_total'], :>, proof['trial_token_cap']
  end

  def test_wire_size_without_budget_exhaustion_is_not_projected
    observation, receipts = reservation_denial_fixture
    observation['request_receipts'].last['request_utf8_bytes'] = 2000
    assert_same observation, @host.budget_validation_view(@trial, observation, receipts)
  end

  def test_fixed_envelope_proof_rejects_changed_wire_hash
    observation, receipts = reservation_denial_fixture
    observation['request_receipts'].first['request_sha256'] = 'changed'
    assert_same observation, @host.budget_validation_view(@trial, observation, receipts)
  end

  def test_fixed_envelope_proof_rejects_changed_reserved_delta
    observation, receipts = reservation_denial_fixture
    receipts.first['reserved_input_tokens'] += 1
    assert_same observation, @host.budget_validation_view(@trial, observation, receipts)
  end

  def test_fixed_envelope_proof_rejects_changed_requested_body
    observation, receipts = reservation_denial_fixture
    receipts.first['requested_body_sha256'] = 'changed'
    assert_same observation, @host.budget_validation_view(@trial, observation, receipts)
  end

  def test_rust_explicit_budget_terminal_with_complete_usage
    @observation['worker_status'] = 'budget_exhausted'
    @observation['runtime'] = {'adapter' => 'rust-worker-native-broker-v1'}
    @observation['did_truncate'] = false
    assert @host.budget_terminal?(@trial, @observation, @ledger.state['requests'])
    @observation['usage_complete'] = false
    refute @host.budget_terminal?(@trial, @observation, @ledger.state['requests'])
  end

  def test_budget_cannot_be_turned_into_quality_score
    AgentRsiLive.write(File.join(@directory_trial, 'verifier.json'), {'passed' => false})
    assert_raises(AgentRsiLive::Denied) { terminal }
  end

  def test_missing_native_evidence_still_blocks
    File.delete(File.join(@directory_trial, 'observation.json'))
    assert_raises(AgentRsiLive::Denied) { terminal }
  end

  def test_failed_process_still_blocks
    AgentRsiLive.write(File.join(@directory, 'known-process.json'), {'exit_status' => 1})
    assert_raises(AgentRsiLive::Denied) { terminal }
  end

  def test_infrastructure_terminal_is_not_automatically_skipped
    @ledger.state['attempts'].first['status'] = 'infrastructure_error'
    assert_raises(AgentRsiLive::Denied) { terminal }
  end

  def test_duplicate_terminal_identity_blocks
    @ledger.state['attempts'] << @ledger.state['attempts'].first.dup
    assert_raises(AgentRsiLive::Denied) { terminal }
  end

  def test_unstarted_trial_is_not_skipped
    refute @host.prior_terminal!(@trial.merge('trial_id' => 'future'))
  end

  def test_altered_trial_identity_blocks
    assert_raises(AgentRsiLive::Denied) { @host.prior_terminal!(@trial.merge('task_id' => 'other')) }
  end

  def test_quality_terminal_requires_original_observation_hash
    @ledger.state['attempts'].first['status'] = 'passed'
    @observation['did_truncate'] = false
    write_observation
    AgentRsiLive.write(File.join(@directory_trial, 'verifier.json'), {'passed' => true, 'observation_sha256' => 'wrong'})
    assert_raises(AgentRsiLive::Denied) { terminal }
    AgentRsiLive.write(File.join(@directory_trial, 'verifier.json'), {'passed' => true, 'observation_sha256' => AgentRsiLive.digest(@observation)})
    assert terminal
  end

  def test_holdout_requires_all_216_planned_terminals_including_budget_evidence
    plan = Array.new(216) { |i| {'trial_id' => i.to_s, 'split' => 'validation'} }
    plan << {'trial_id' => 'holdout', 'split' => 'holdout'}
    @host.stub(:prior_terminal!, true) { @host.holdout_ready!(plan) }
    @host.stub(:prior_terminal!, ->(trial) { trial['trial_id'] != '215' }) do
      assert_raises(AgentRsiLive::Denied) { @host.holdout_ready!(plan) }
    end
    @host.stub(:prior_terminal!, true) do
      assert_raises(AgentRsiLive::Denied) { @host.holdout_ready!(plan.drop(1)) }
      duplicate = plan.map(&:dup)
      duplicate[1]['trial_id'] = duplicate[0]['trial_id']
      assert_raises(AgentRsiLive::Denied) { @host.holdout_ready!(duplicate) }
    end
  end

  def test_pairing_never_combines_scoring_epochs
    rows = %w[baseline candidate].flat_map do |arm|
      (1..3).map { |repetition| {'platform' => 'rust', 'task_id' => 'same', 'arm' => arm, 'repetition' => repetition,
        'status' => 'passed', 'scoring_epoch' => arm == 'baseline' ? 'v1-historical' : 'v2-prospective',
        'verification' => {'classification_correct' => true}} }
    end
    assert @host.paired_comparison(rows).all? { |row| row['independent_task_pairs'] == 0 }
  end
end
