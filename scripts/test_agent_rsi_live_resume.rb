#!/usr/bin/env ruby
# frozen_string_literal: true
require 'minitest/autorun'
require 'tmpdir'
require_relative 'lib/agent_rsi_live'

module AgentRsiResumeReporting
  class << self
    attr_accessor :reporter
    def minitest_plugin_init(_options)
      self.reporter = Minitest.reporter
    end
  end
end
Minitest.extensions << AgentRsiResumeReporting
report_index = ARGV.index('--report-directory')
if report_index
  report_directory = File.expand_path(ARGV.fetch(report_index + 1))
  ARGV.slice!(report_index, 2)
  Minitest.after_run do
    summary = AgentRsiResumeReporting.reporter.reporters.find { |reporter| reporter.is_a?(Minitest::SummaryReporter) }
    result = {'tests' => summary.count, 'assertions' => summary.assertions, 'failures' => summary.failures,
              'errors' => summary.errors, 'skipped' => summary.skips, 'paid_model_calls' => 0,
              'passed' => summary.failures.zero? && summary.errors.zero?}
    FileUtils.mkdir_p(report_directory, mode: 0o700)
    AgentRsiLive.write(File.join(report_directory, 'resume-ledger.json'), result)
    AgentRsiLive.write(File.join(report_directory, 'resume-ledger.md'),
                      "# 预算账本显式恢复免费测试\n\n#{result['tests']} 项，#{result['assertions']} 个断言，失败 #{result['failures']}，错误 #{result['errors']}。\n\n真实模型请求：0。未读取或修改真实账本。\n")
  end
end

class AgentRsiLiveResumeTest < Minitest::Test
  def setup
    @directory = Dir.mktmpdir('rsi-resume-free-')
    @manifest = {'proposal_sha256' => AgentRsiLive::PROPOSAL, 'run_directory' => File.realpath(@directory),
                 'account' => {'scope_sha256' => 'same-account-scope'},
                 'maximum_bill_microusd_per_million_tokens' => 694_050, 'expected_routed_model' => 'fixed-route'}
    AgentRsiLive.write(File.join(@directory, 'manifest.json'), @manifest)
    @ledger = AgentRsiLive::Ledger.new(@directory, @manifest)
    @trial = {'trial_id' => 'original', 'platform' => 'rust-native-worker', 'task_id' => 'original-task',
              'split' => 'validation', 'arm' => 'baseline', 'repetition' => 1, 'attempt' => 1,
              'resume_of' => nil, 'agent_revision' => AgentRsiLive::REVISIONS['baseline']}
    @body = {'model' => AgentRsiLive::MODEL, 'stream' => false, 'temperature' => 0, 'max_tokens' => 4000,
             'messages' => [{'role' => 'user', 'content' => 'fixture ' * 300}]}
    @ledger.begin_trial(@trial)
    record, = @ledger.authorize_request('original', 1, @body)
    @ledger.settle(record, {'id' => 'response-original', 'usage' => {'prompt_tokens' => 1457, 'completion_tokens' => 72, 'total_tokens' => 1529}},
                   {'x-request-id' => 'request-original', 'x-routed-model' => 'fixed-route'})
    record.merge!('accounted_microusd' => 776, 'observed_bill_microusd' => 776, 'verified_bill_microusd' => 776,
                  'actual_pricing_snapshot' => {'input_price' => 0.221, 'cost_margin' => 1.05, 'billing_multiplier' => 2})
    @ledger.finish_trial('infrastructure_error')
    @ledger.pause('waiting for an explicit signed amendment')
    close_ledger
  end

  def teardown
    close_ledger
    FileUtils.remove_entry(@directory)
  end

  def close_ledger
    @ledger&.close
    @ledger = nil
  end

  def ledger_path = File.join(@directory, 'ledger.json')
  def raw_digest = Digest::SHA256.file(ledger_path).hexdigest
  def persisted = JSON.parse(File.binread(ledger_path))
  def rewrite
    state = persisted
    yield state
    AgentRsiLive.write(ledger_path, state)
  end

  def changed_manifest
    Marshal.load(Marshal.dump(@manifest)).merge('maximum_bill_microusd_per_million_tokens' => 1_388_100,
                                               'budget_cap_microusd' => 20_000_000)
  end

  def resume(manifest = changed_manifest, digest: raw_digest, expires_at: Time.now + 60)
    @ledger = AgentRsiLive::Ledger.new(@directory, manifest, resume_from_sha256: digest, expires_at: expires_at)
  end

  def test_default_constructor_still_cannot_overwrite_or_resume
    before = File.binread(ledger_path)
    assert_raises(AgentRsiLive::Denied) { AgentRsiLive::Ledger.new(@directory, changed_manifest) }
    assert_equal before, File.binread(ledger_path)
    resume
    assert_nil @ledger.state['paused']
  end

  def test_raw_digest_must_match_and_rejected_attempt_releases_lock
    before = File.binread(ledger_path)
    assert_raises(AgentRsiLive::Denied) { resume(digest: 'a' * 64) }
    assert_equal before, File.binread(ledger_path)
    resume
    assert_equal 1, @ledger.state['attempts'].length
  end

  def test_explicit_resume_preserves_all_old_rows_and_counters
    rewrite { |state| state.merge!('holdout_accesses' => 7, 'retries' => 2, 'holdout_retries' => 1) }
    before = persisted
    resume
    %w[attempts requests holdout_accesses retries holdout_retries frozen_routed_model].each do |key|
      assert_equal before[key], @ledger.state[key]
    end
    assert_nil @ledger.state['paused']
    assert_equal 776, AgentRsiLive::Ledger.exposure(@ledger.state['requests'].first)
    assert_equal 1529, @ledger.state['requests'].sum { |row| row['actual_tokens'] }
  end

  def test_snapshots_bind_original_raw_ledger_and_manifest_and_new_ledger
    before = File.binread(ledger_path)
    before_sha = raw_digest
    original_manifest = File.binread(File.join(@directory, 'manifest.json'))
    resume
    amendment = @ledger.state['amendments'].last
    assert_equal before, File.binread(File.join(@directory, amendment['ledger_snapshot']))
    assert_equal before_sha, amendment['resume_from_sha256']
    assert_equal original_manifest, File.binread(File.join(@directory, amendment['previous_manifest_snapshot']))
    assert_equal AgentRsiLive.digest(changed_manifest), @ledger.state['manifest_sha256']
    receipt = JSON.parse(File.binread(File.join(@directory, amendment['resume_receipt'])))
    assert_equal before_sha, receipt['previous_ledger_sha256']
    assert_equal raw_digest, receipt['amended_ledger_sha256']
    assert Time.iso8601(amendment['amended_at'])
  end

  def test_multiple_amendments_form_a_chain_without_counter_reset
    resume
    first = @ledger.state['amendments'].last.dup
    @ledger.pause('another separately signed amendment')
    close_ledger
    next_manifest = changed_manifest.merge('amendment_note' => 'second authorized change')
    resume(next_manifest)
    assert_equal 2, @ledger.state['amendments'].length
    assert_equal first, @ledger.state['amendments'].first
    assert_equal AgentRsiLive.digest(first), @ledger.state['amendments'].last['previous_amendment_sha256']
    assert_equal 1, @ledger.state['requests'].length
    assert_equal 776, AgentRsiLive::Ledger.exposure(@ledger.state['requests'].first)
  end

  def test_unknown_or_reserved_requests_block_resume_even_with_usage
    original = persisted
    %w[unknown reserved].each do |status|
      state = Marshal.load(Marshal.dump(original))
      state['requests'].first['status'] = status
      AgentRsiLive.write(ledger_path, state)
      before = File.binread(ledger_path)
      assert_raises(AgentRsiLive::Denied) { resume }
      assert_equal before, File.binread(ledger_path)
    end
  end

  def test_running_trial_blocks_resume
    rewrite { |state| state['attempts'].first['status'] = 'running' }
    assert_raises(AgentRsiLive::Denied) { resume }
    assert_equal 'running', persisted['attempts'].first['status']
    assert persisted['paused']
  end

  def test_unverified_bill_and_unknown_tokens_block_resume
    original = persisted
    %w[verified_bill_microusd actual_tokens accounted_microusd].each do |field|
      state = Marshal.load(Marshal.dump(original))
      state['requests'].first.delete(field)
      AgentRsiLive.write(ledger_path, state)
      assert_raises(AgentRsiLive::Denied) { resume }
    end
  end

  def test_changed_account_or_directory_is_rejected_before_pause_is_cleared
    account = changed_manifest.merge('account' => {'scope_sha256' => 'different-account'})
    assert_raises(AgentRsiLive::Denied) { resume(account) }
    directory = changed_manifest.merge('run_directory' => Dir.tmpdir)
    assert_raises(AgentRsiLive::Denied) { resume(directory) }
    assert persisted['paused']
  end

  def test_old_digest_is_not_replayable_after_an_amendment
    original_sha = raw_digest
    resume
    close_ledger
    assert_raises(AgentRsiLive::Denied) { resume(digest: original_sha) }
    assert_equal 1, persisted['amendments'].length
  end

  def test_resume_does_not_promote_incomplete_trial_to_retry_eligibility
    rewrite { |state| state['attempts'].first['status'] = 'incomplete' }
    resume
    retry_trial = @trial.merge('trial_id' => 'retry', 'attempt' => 2, 'resume_of' => 'original')
    assert_raises(AgentRsiLive::Denied) { @ledger.begin_trial(retry_trial) }
    assert_equal 0, @ledger.state['retries']
    assert_equal 'incomplete', @ledger.state['attempts'].first['status']
  end

  def test_existing_infrastructure_retry_is_allowed_once_and_consumes_old_budget
    resume
    retry_trial = @trial.merge('trial_id' => 'retry', 'attempt' => 2, 'resume_of' => 'original')
    @ledger.begin_trial(retry_trial)
    record, = @ledger.authorize_request('retry', 1, @body)
    assert_operator AgentRsiLive::Ledger.exposure(record), :>, 0
    assert_equal 776 + record['reserved_microusd'], @ledger.state['requests'].sum { |row| AgentRsiLive::Ledger.exposure(row) }
    @ledger.finish_trial('infrastructure_error')
    assert_raises(AgentRsiLive::Denied) { @ledger.begin_trial(retry_trial.merge('trial_id' => 'another-retry')) }
    assert_equal 1, @ledger.state['retries']
  end

  def test_twenty_dollar_cap_is_total_including_old_spend
    rewrite { |state| state['requests'].first.merge!('accounted_microusd' => 5_000_001, 'observed_bill_microusd' => 5_000_001, 'verified_bill_microusd' => 5_000_001) }
    assert_raises(AgentRsiLive::Denied) { resume(@manifest) }
    resume
    assert_equal 20_000_000, @ledger.state['budget_cap_microusd']
    amendment = @ledger.state['amendments'].last
    assert_equal 5_000_000, amendment['previous_budget_cap_microusd']
    assert_equal 20_000_000, amendment['budget_cap_microusd']
    @ledger.begin_trial(@trial.merge('trial_id' => 'next', 'task_id' => 'next-task'))
    row, = @ledger.authorize_request('next', 1, @body)
    assert_equal 5_000_001 + row['reserved_microusd'], @ledger.state['requests'].sum { |r| AgentRsiLive::Ledger.exposure(r) }
  end

  def test_twenty_dollar_cap_cannot_reset_old_spend_near_cap
    rewrite { |state| state['requests'].first.merge!('accounted_microusd' => 19_999_999, 'observed_bill_microusd' => 19_999_999, 'verified_bill_microusd' => 19_999_999) }
    resume
    @ledger.begin_trial(@trial.merge('trial_id' => 'next', 'task_id' => 'next-task'))
    assert_raises(AgentRsiLive::Denied) { @ledger.authorize_request('next', 1, @body) }
    assert_equal 1, @ledger.state['requests'].length
  end

  def test_invalid_or_more_than_twenty_dollar_cap_is_denied
    [0, -1, 20_000_001, 20_000_000.0, '20000000'].each do |cap|
      assert_raises(AgentRsiLive::Denied) { resume(changed_manifest.merge('budget_cap_microusd' => cap)) }
    end
    resume
    assert_equal 776, @ledger.state['requests'].first['verified_bill_microusd']
  end

  def test_expiration_still_blocks_each_request_after_resume
    resume
    @ledger.begin_trial(@trial.merge('trial_id' => 'next', 'task_id' => 'next-task'))
    @ledger.instance_variable_set(:@expires_at, Time.now - 1)
    assert_raises(AgentRsiLive::Denied) { @ledger.authorize_request('next', 1, @body) }
    assert_equal 1, @ledger.state['requests'].length
  end

  def test_expired_resume_cannot_clear_pause
    assert_raises(AgentRsiLive::Denied) { resume(expires_at: Time.now - 1) }
    assert persisted['paused']
  end

  def test_existing_holdout_and_retry_quotas_are_not_refunded
    rewrite { |state| state.merge!('holdout_accesses' => 90, 'retries' => 72, 'holdout_retries' => 18) }
    resume
    assert_raises(AgentRsiLive::Denied) { @ledger.begin_trial(@trial.merge('trial_id' => 'holdout', 'task_id' => 'holdout', 'split' => 'holdout')) }
    assert_equal 90, @ledger.state['holdout_accesses']
    assert_raises(AgentRsiLive::Denied) { @ledger.begin_trial(@trial.merge('trial_id' => 'retry', 'attempt' => 2, 'resume_of' => 'original')) }
    assert_equal 72, @ledger.state['retries']
  end

  def test_live_pricing_floats_remain_unchanged_and_raw_snapshot_is_exact
    before = File.binread(ledger_path)
    resume
    assert_equal({'input_price' => 0.221, 'cost_margin' => 1.05, 'billing_multiplier' => 2}, @ledger.state['requests'].first['actual_pricing_snapshot'])
    snapshot = @ledger.state['amendments'].last['ledger_snapshot']
    assert_equal before, File.binread(File.join(@directory, snapshot))
  end

  def test_unknown_billing_keeps_reserved_exposure_even_after_usage_is_known
    row = {'status' => 'unknown', 'reserved_microusd' => 5000, 'accounted_microusd' => 100,
           'observed_bill_microusd' => 200, 'verified_bill_microusd' => 300}
    assert_equal 5000, AgentRsiLive::Ledger.exposure(row)
    row['status'] = 'reserved'
    assert_equal 5000, AgentRsiLive::Ledger.exposure(row)
    row['status'] = 'settled'
    assert_equal 300, AgentRsiLive::Ledger.exposure(row)
  end
end
