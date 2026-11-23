#!/usr/bin/env ruby
# frozen_string_literal: true

# Offline only: API, monotonic clock and sleeper are injected. No token or network.
require 'fileutils'
require 'json'
require 'minitest/autorun'
require 'minitest/mock'
require 'yaml'
require_relative '../ci_self_hosted_attempt'

module CiRunnerContracts
  ROOT = File.expand_path('../..', __dir__)
  SHA = 'a' * 40
  NONCE = 'b' * 32
  PARENT = 123
  CHILD = 456
  REPORT_INDEX = ARGV.index('--report-dir')
  REPORT_DIR = if REPORT_INDEX
                 value = ARGV[REPORT_INDEX + 1]
                 raise ArgumentError, '--report-dir needs a directory' if value.nil? || value.start_with?('-')

                 ARGV.slice!(REPORT_INDEX, 2)
                 File.expand_path(value)
               end

  class Reporter < Minitest::StatisticsReporter
    def initialize(directory)
      super($stdout, {})
      @directory = directory
      @rows = []
    end

    def record(result)
      super
      @rows << { name: result.name, status: result.passed? ? 'passed' : 'failed',
                 assertions: result.assertions, failures: result.failures.map(&:message) }
    end

    def report
      FileUtils.mkdir_p(@directory)
      File.write(File.join(@directory, 'report.json'), JSON.pretty_generate({
        mode: 'offline_ci_runner_contracts', tests: @rows.length, assertions: @rows.sum { |row| row[:assertions] },
        failures: @rows.count { |row| row[:status] != 'passed' }, rows: @rows
      }) + "\n")
      text = ["# CI runner 离线契约回归", '', '仅使用模拟 API 和时钟；没有派发远端工作流或运行完整产品测试。', '',
              '| 用例 | 结果 | 断言数 |', '| --- | --- | --- |']
      @rows.each { |row| text << "| #{row[:name]} | #{row[:status]} | #{row[:assertions]} |" }
      File.write(File.join(@directory, 'report.md'), text.join("\n") + "\n")
    end
  end

  Minitest.extensions << 'ci_runner_report' if REPORT_DIR
  def Minitest.plugin_ci_runner_report_init(_options)
    reporter << CiRunnerContracts::Reporter.new(CiRunnerContracts::REPORT_DIR)
  end

  class FakeApi
    attr_reader :calls, :time
    attr_accessor :dispatch_status, :scenario, :fault, :cancel_delay, :discoverable, :other_runs, :os

    def initialize(os = 'ubuntu-latest')
      @os = os
      @calls = []
      @time = 0.0
      @dispatch_status = 200
      @cancel_delay = 0
      @discoverable = true
      @other_runs = []
      @scenario = ->(_time) { {} }
    end

    def sleep(seconds)
      @time += seconds
    end

    def state
      @scenario.call(@time)
    end

    def run
      data = {
        'id' => CHILD, 'event' => 'workflow_dispatch', 'head_branch' => 'feature/ci', 'head_sha' => SHA,
        'display_title' => "ci-worker-#{PARENT}-2-#{@os}-#{NONCE}",
        'path' => CiSelfHostedAttempt::WORKFLOW, 'repository' => { 'full_name' => 'example/project' },
        'run_attempt' => 1, 'status' => state.fetch(:run_status, 'completed'), 'conclusion' => 'success'
      }.merge(state.fetch(:run, {}))
      if @cancel_at && @cancel_delay && @time - @cancel_at >= @cancel_delay
        data.merge!('status' => 'completed', 'conclusion' => 'cancelled')
      end
      data
    end

    def job
      return if state[:no_job]

      steps = CiSelfHostedAttempt::REQUIRED_STEPS.map.with_index do |name, index|
        { 'name' => name, 'number' => index + 1, 'status' => 'completed', 'conclusion' => 'success' }
      end
      {
        'id' => 789, 'run_id' => CHILD, 'run_attempt' => 1, 'head_sha' => SHA,
        'name' => "CI self-hosted worker (#{@os})", 'labels' => CiSelfHostedAttempt::PLATFORMS.fetch(@os),
        'runner_id' => 321, 'runner_name' => 'offline-runner',
        'status' => 'completed', 'conclusion' => 'success', 'steps' => steps
      }.merge(state.fetch(:job, {}))
    end

    def request(method, path, body = nil)
      @calls << { time: @time, method: method, path: path, body: body }
      @fault&.call(method, path)
      data = case path
             when %r{/workflows/#{Regexp.escape(File.basename(CiSelfHostedAttempt::WORKFLOW))}/dispatches\z}
               @request = body
               return CiSelfHostedAttempt::Response.new(status: @dispatch_status, data: { 'workflow_run_id' => CHILD })
             when %r{/workflows/#{Regexp.escape(File.basename(CiSelfHostedAttempt::WORKFLOW))}/runs\?}
               { 'workflow_runs' => @other_runs + (@discoverable ? [run] : []) }
             when %r{/runs/#{PARENT}\z}
               { 'status' => state.fetch(:parent_status, 'in_progress') }
             when %r{/runs/#{CHILD}/attempts/1/jobs\?}
               jobs = [job].compact
               { 'total_count' => jobs.length, 'jobs' => jobs }
             when %r{/runs/#{CHILD}\z}
               run
             when %r{/runs/#{CHILD}/(?:cancel|force-cancel)\z}
               @cancel_at ||= @time
               {}
             else
               raise "unexpected API request #{method} #{path}"
             end
      CiSelfHostedAttempt::Response.new(status: method == :post ? 202 : 200, data: data)
    end
  end
end

class CiSelfHostedAttemptTest < Minitest::Test
  include CiRunnerContracts

  def setup
    @api = FakeApi.new
  end

  def attempt(**overrides)
    coordinator = CiSelfHostedAttempt::Coordinator.new(**{
      api: @api, repo: 'example/project', ref: 'refs/heads/feature/ci', sha: SHA, os: @api.os,
      parent_id: PARENT, parent_attempt: 2, nonce: NONCE,
      clock: -> { @api.time }, sleeper: ->(seconds) { @api.sleep(seconds) }
    }.merge(overrides))
    CiSelfHostedAttempt.with_signal_handlers(coordinator) { coordinator.attempt }
  end

  def cancellations
    @api.calls.select { |call| call[:path].match?(%r{/(?:cancel|force-cancel)\z}) }
  end

  def test_complete_self_hosted_success_avoids_hosted_on_both_platforms
    CiSelfHostedAttempt::PLATFORMS.each_key do |os|
      @api = FakeApi.new(os)
      result = attempt
      assert result.passed, result.reason
      assert_empty cancellations
      dispatch = @api.calls.first
      assert_equal 'feature/ci', dispatch.dig(:body, 'ref')
      assert_equal SHA, dispatch.dig(:body, 'inputs', 'expected_sha')
      assert_equal "#{PARENT}-2-#{os}-#{NONCE}", dispatch.dig(:body, 'inputs', 'request_id')
    end
  end

  def test_run_in_progress_with_no_job_is_still_subject_to_queue_budget
    @api.scenario = ->(_time) { { run_status: 'in_progress', no_job: true } }
    result = attempt
    refute result.passed
    assert_match(/queue exceeded 300/, result.reason)
    assert_equal 300, cancellations.first[:time]
    assert_equal 330, @api.time
    assert_match(/confirmed/, result.cleanup)
  end

  def test_queued_job_does_not_start_execution_budget
    @api.scenario = ->(_time) { { run_status: 'in_progress', job: { 'status' => 'queued' } } }
    result = attempt
    refute result.passed
    assert_match(/queue/, result.reason)
    assert_equal 300, cancellations.first[:time]
  end

  def test_acceptance_near_queue_deadline_gets_its_own_execution_budget
    @api.scenario = lambda do |time|
      if time < 270
        { run_status: 'in_progress', no_job: true }
      elsif time < 600
        { run_status: 'in_progress', job: { 'status' => 'in_progress' } }
      else
        {}
      end
    end
    assert attempt.passed
    assert_equal 600, @api.time
    assert_empty cancellations
  end

  def test_running_job_times_out_and_is_cancelled
    @api.scenario = ->(_time) { { run_status: 'in_progress', job: { 'status' => 'in_progress' } } }
    result = attempt
    refute result.passed
    assert_match(/execution exceeded 3600/, result.reason)
    assert_equal 3600, cancellations.first[:time]
  end

  def test_failure_after_acceptance_retries_on_hosted
    @api.scenario = lambda do |time|
      time.zero? ? { run_status: 'in_progress', job: { 'status' => 'in_progress' } } :
                   { job: { 'conclusion' => 'failure' } }
    end
    result = attempt
    refute result.passed
    assert_match(/full hosted suite/, result.reason)
    assert_equal 30, @api.time
  end

  def test_run_success_and_job_success_cannot_mask_failed_or_missing_steps
    %w[Set\ up\ job Validate\ worker\ identity Checkout\ expected\ revision Run\ complete\ CI\ suite Self-hosted\ suite\ passed].each do |name|
      %i[missing failure skipped].each do |fault|
        @api = FakeApi.new
        steps = @api.job.fetch('steps')
        if fault == :missing
          steps.reject! { |step| step['name'] == name }
        else
          steps.find { |step| step['name'] == name }['conclusion'] = fault.to_s
        end
        @api.scenario = ->(_time) { { job: { 'steps' => steps } } }
        refute attempt.passed, "must retry for #{name} #{fault}"
      end
    end
  end

  def test_any_extra_failed_step_rejects_success_evidence
    steps = @api.job.fetch('steps') + [{ 'name' => 'Post cleanup', 'status' => 'completed', 'conclusion' => 'failure' }]
    @api.scenario = ->(_time) { { job: { 'steps' => steps } } }
    refute attempt.passed
  end

  def test_worker_job_identity_must_match
    [{ 'head_sha' => 'c' * 40 }, { 'run_id' => PARENT }, { 'run_attempt' => 2 },
     { 'labels' => ['ubuntu-latest'] }, { 'runner_id' => 0 }, { 'runner_name' => '' },
     { 'name' => 'other job' }, { 'conclusion' => 'failure' }].each do |change|
      @api = FakeApi.new
      @api.scenario = ->(_time) { { job: change } }
      refute attempt.passed, change.inspect
    end
  end

  def test_204_response_discovers_only_exact_nonce_branch_and_event
    @api.dispatch_status = 204
    @api.other_runs = [@api.run.merge('id' => 900, 'display_title' => 'ci-worker-someone-else')]
    assert attempt.passed
    discovery = @api.calls.find { |call| call[:path].include?("/workflows/#{File.basename(CiSelfHostedAttempt::WORKFLOW)}/runs?") }
    query = URI.decode_www_form(URI("https://example.test#{discovery[:path]}").query).to_h
    assert_equal 'workflow_dispatch', query['event']
    assert_equal 'feature/ci', query['branch']
    assert_empty cancellations
  end

  def test_unfindable_204_is_bounded_and_never_cancels_unrelated_runs
    @api.dispatch_status = 204
    @api.discoverable = false
    @api.other_runs = [@api.run.merge('id' => 900, 'display_title' => 'unrelated')]
    result = attempt
    refute result.passed
    assert_equal 300, @api.time
    assert_empty cancellations
    assert_match(/not discoverable/, result.cleanup)
  end

  def test_changed_head_or_other_run_is_never_cancelled
    [{ 'head_sha' => 'c' * 40 }, { 'id' => PARENT }, { 'id' => 999 }, { 'event' => 'push' },
     { 'head_branch' => 'main' }, { 'path' => '.github/workflows/release.yml' },
     { 'display_title' => 'different-parent-or-os' }, { 'run_attempt' => 2 },
     { 'repository' => { 'full_name' => 'fork/project' } }].each do |change|
      @api = FakeApi.new
      @api.scenario = ->(_time) { { run_status: 'in_progress', run: change } }
      result = attempt
      refute result.passed, change.inspect
      assert_empty cancellations, change.inspect
      assert_match(/identity|HEAD/, result.reason)
    end
  end

  def test_ambiguous_nonce_fails_closed_without_cancellation
    @api.dispatch_status = 204
    @api.other_runs = [@api.run.merge('id' => 999)]
    result = attempt
    refute result.passed
    assert_empty cancellations
    assert_match(/ambiguous/, result.reason)
  end

  def test_api_403_rate_limit_and_5xx_fail_closed
    [403, 429, 500, 503].each do |status|
      @api = FakeApi.new
      @api.fault = ->(method, path) { raise CiSelfHostedAttempt::ApiError.new(method, path, status) }
      result = attempt
      refute result.passed
      assert_match(/HTTP #{status}/, result.reason)
      assert_match(/could not be confirmed/, result.cleanup)
      assert_empty cancellations
    end
  end

  def test_poll_api_failure_still_cleans_up_verified_child
    @api.scenario = ->(_time) { { run_status: 'in_progress', job: { 'status' => 'in_progress' } } }
    @api.fault = lambda do |method, path|
      raise CiSelfHostedAttempt::ApiError.new(method, path, 503) if path.include?('/jobs?')
    end
    result = attempt
    refute result.passed
    assert_match(/HTTP 503/, result.reason)
    assert_equal 1, cancellations.length
    assert_match(/confirmed/, result.cleanup)
  end

  def test_first_dispatch_not_registered_uses_hosted_with_bootstrap_explanation
    @api.fault = lambda do |method, path|
      raise CiSelfHostedAttempt::ApiError.new(method, path, 422) if path.end_with?('/dispatches')
    end
    @api.discoverable = false
    result = attempt
    refute result.passed
    assert_match(/default branch/, result.reason)
    assert_match(/until merged/, result.reason)
    assert_empty cancellations
  end

  def test_lost_dispatch_response_can_find_and_cancel_own_child
    @api.scenario = ->(_time) { { run_status: 'in_progress', no_job: true } }
    @api.fault = ->(_method, path) { raise CiSelfHostedAttempt::Error, 'API timeout' if path.end_with?('/dispatches') }
    result = attempt
    refute result.passed
    assert_equal 1, cancellations.length
    assert_equal '/repos/example/project/actions/runs/456/cancel', cancellations.first[:path]
  end

  def test_parent_cancel_status_cleans_up_own_child
    @api.scenario = lambda do |time|
      { parent_status: time.zero? ? 'in_progress' : 'completed', run_status: 'in_progress', no_job: true }
    end
    result = attempt
    refute result.passed
    assert_match(/parent cancelled/, result.reason)
    assert_equal 30, cancellations.first[:time]
    assert_equal ['force-cancel'], cancellations.map { |call| call[:path].split('/').last }
    assert_equal 30, @api.time
  end

  def test_parent_cancel_signal_while_sleeping_cleans_up_own_child
    @api.scenario = ->(_time) { { run_status: 'in_progress', no_job: true } }
    first_sleep = true
    sleeper = lambda do |seconds|
      if first_sleep
        first_sleep = false
        raise CiSelfHostedAttempt::ParentCancelled, 'simulated TERM'
      end
      @api.sleep(seconds)
    end
    result = attempt(sleeper: sleeper)
    refute result.passed
    assert_equal 1, cancellations.length
    assert_match(/confirmed/, result.cleanup)
  end

  def test_real_int_then_repeated_term_and_int_cannot_interrupt_parent_cleanup
    @api.scenario = ->(_time) { { run_status: 'in_progress', no_job: true } }
    @api.fault = lambda do |_method, path|
      next unless path.end_with?('/force-cancel')

      %w[TERM INT TERM].each { |signal| Process.kill(signal, Process.pid) }
    end
    result = attempt(sleeper: ->(_seconds) { Process.kill('INT', Process.pid) })
    refute result.passed
    assert_match(/parent cancelled/, result.reason)
    assert_equal ['force-cancel'], cancellations.map { |call| call[:path].split('/').last }
    assert_equal 0, @api.time # No sleep in cancellation cleanup.
    force_index = @api.calls.index(cancellations.first)
    assert_equal "/repos/example/project/actions/runs/#{CHILD}", @api.calls[force_index - 1][:path]
    assert_match(/completion confirmed/, result.cleanup)
  end

  def test_first_signal_during_ordinary_cleanup_switches_to_short_cleanup
    @api.scenario = ->(_time) { { run_status: 'in_progress', no_job: true } }
    @api.fault = lambda do |_method, path|
      Process.kill('INT', Process.pid) if path.end_with?('/cancel')
      Process.kill('TERM', Process.pid) if path.end_with?('/force-cancel')
    end
    result = attempt
    refute result.passed
    assert_equal %w[cancel force-cancel], cancellations.map { |call| call[:path].split('/').last }
    assert_equal 300, @api.time
    assert_match(/completion confirmed/, result.cleanup)
  end

  def test_parent_cleanup_has_five_second_total_budget_even_if_api_hangs
    @api.scenario = ->(_time) { { parent_status: 'completed', run_status: 'in_progress', no_job: true } }
    @api.fault = lambda do |_method, path|
      # Spend part of the budget verifying, then hang in force-cancel. The two
      # requests must share one deadline rather than each receiving five seconds.
      Kernel.sleep(2) if path.end_with?("/runs/#{CHILD}")
      Kernel.sleep(20) if path.end_with?('/force-cancel')
    end
    assert_equal 5, CiSelfHostedAttempt::PARENT_CLEANUP_SECONDS
    started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    result = attempt
    elapsed = Process.clock_gettime(Process::CLOCK_MONOTONIC) - started
    refute result.passed
    assert_operator elapsed, :>=, 4.5
    assert_operator elapsed, :<, 7
    assert_equal ['force-cancel'], cancellations.map { |call| call[:path].split('/').last }
    assert_match(/budget exhausted.*not confirmed/, result.cleanup)
  end

  def test_parent_cleanup_api_failure_and_pending_force_do_not_claim_confirmation
    [503, nil].each do |status|
      @api = FakeApi.new
      @api.cancel_delay = nil
      @api.scenario = ->(_time) { { parent_status: 'completed', run_status: 'in_progress', no_job: true } }
      @api.fault = lambda do |method, path|
        raise CiSelfHostedAttempt::ApiError.new(method, path, status) if status && path.end_with?('/force-cancel')
      end
      result = attempt
      refute result.passed
      assert_equal ['force-cancel'], cancellations.map { |call| call[:path].split('/').last }
      assert_equal 0, @api.time
      assert_match(/not (?:be )?confirmed/, result.cleanup)
    end
  end

  def test_parent_cleanup_never_force_cancels_unverified_or_missing_worker
    [{ 'head_sha' => 'c' * 40 }, { 'path' => '.github/workflows/ci.yml' }, { 'id' => PARENT }].each do |change|
      @api = FakeApi.new
      @api.scenario = ->(_time) { { parent_status: 'completed', run_status: 'in_progress', run: change } }
      result = attempt
      refute result.passed
      assert_empty cancellations
      assert_match(/could not be confirmed/, result.cleanup)
    end
    @api = FakeApi.new
    @api.dispatch_status = 204
    @api.discoverable = false
    @api.scenario = ->(_time) { { parent_status: 'completed' } }
    result = attempt
    assert_empty cancellations
    assert_match(/no cancellation confirmed/, result.cleanup)
  end

  def test_force_cancel_when_normal_cancel_does_not_stop_child
    @api.cancel_delay = 45
    @api.scenario = ->(_time) { { run_status: 'in_progress', no_job: true } }
    result = attempt
    refute result.passed
    assert_equal %w[cancel force-cancel], cancellations.map { |call| call[:path].split('/').last }
    assert_equal 345, @api.time
    assert_match(/confirmed/, result.cleanup)
  end

  def test_normal_cancel_conflict_rechecks_identity_and_force_cancels
    @api.scenario = ->(_time) { { run_status: 'in_progress', no_job: true } }
    @api.fault = lambda do |method, path|
      raise CiSelfHostedAttempt::ApiError.new(method, path, 409) if path.end_with?('/cancel')
    end
    result = attempt
    refute result.passed
    assert_equal %w[cancel force-cancel], cancellations.map { |call| call[:path].split('/').last }
    assert_match(/confirmed/, result.cleanup)
  end

  def test_normal_cancel_conflict_never_force_cancels_changed_identity
    @api.scenario = ->(_time) { { run_status: 'in_progress', no_job: true } }
    @api.fault = lambda do |method, path|
      if path.end_with?('/cancel')
        @api.scenario = ->(_time) { { run_status: 'in_progress', run: { 'head_sha' => 'c' * 40 } } }
        raise CiSelfHostedAttempt::ApiError.new(method, path, 409)
      end
    end
    result = attempt
    refute result.passed
    assert_equal ['cancel'], cancellations.map { |call| call[:path].split('/').last }
    assert_match(/could not be confirmed/, result.cleanup)
  end

  def test_failed_worker_cancels_even_before_run_aggregation_finishes
    @api.scenario = ->(_time) { { run_status: 'in_progress', job: { 'conclusion' => 'failure' } } }
    result = attempt
    refute result.passed
    assert_equal 0, cancellations.first[:time]
    assert_match(/full hosted suite/, result.reason)
  end

  def test_cleanup_api_failure_does_not_block_hosted_or_claim_cancellation
    @api.scenario = ->(_time) { { run_status: 'in_progress', no_job: true } }
    @api.fault = lambda do |method, path|
      raise CiSelfHostedAttempt::ApiError.new(method, path, 403) if path.end_with?('/cancel')
    end
    result = attempt
    refute result.passed
    assert_match(/queue exceeded/, result.reason)
    assert_match(/could not be confirmed.*HTTP 403/, result.cleanup)
  end

  def test_transport_exception_fails_closed
    @api.fault = ->(_method, _path) { raise CiSelfHostedAttempt::Error, 'API transport failed (Net::ReadTimeout)' }
    result = attempt
    refute result.passed
    assert_match(/transport failed/, result.reason)
    assert_empty cancellations
  end

  def test_cleanup_failure_is_not_claimed_as_success
    @api.cancel_delay = nil
    @api.scenario = ->(_time) { { run_status: 'in_progress', no_job: true } }
    result = attempt
    refute result.passed
    assert_equal 345, @api.time
    assert_match(/not confirmed/, result.cleanup)
  end

  def test_identity_is_rechecked_before_force_cancel
    @api.cancel_delay = nil
    @api.scenario = lambda do |time|
      { run_status: 'in_progress', no_job: true, run: time > 300 ? { 'head_sha' => 'c' * 40 } : {} }
    end
    result = attempt
    refute result.passed
    assert_equal ['cancel'], cancellations.map { |call| call[:path].split('/').last }
    assert_match(/could not be confirmed/, result.cleanup)
  end

  def test_polling_uses_injected_monotonic_time_with_30_second_intervals
    @api.scenario = ->(_time) { { run_status: 'in_progress', no_job: true } }
    attempt
    parent_polls = @api.calls.select { |call| call[:path].end_with?("/runs/#{PARENT}") }
    assert_equal (0...300).step(30).to_a, parent_polls.map { |call| call[:time] }
  end

  def test_two_platform_65_minute_request_budget_leaves_api_headroom
    calls = []
    CiSelfHostedAttempt::PLATFORMS.each_key do |os|
      @api = FakeApi.new(os)
      @api.dispatch_status = 204 # Include workflow-list discovery as well as run/job reads.
      @api.cancel_delay = nil
      @api.scenario = lambda do |time|
        { run_status: 'in_progress', job: { 'status' => time < 270 ? 'queued' : 'in_progress' } }
      end
      result = attempt
      refute result.passed
      assert_match(/execution exceeded 3600/, result.reason)
      assert_equal 3870, cancellations.first[:time]
      calls.concat(@api.calls)
    end
    assert_operator CiSelfHostedAttempt::POLL_SECONDS, :>=, 30
    # Conservatively extend the observed queue+execution polls to all 65 minutes.
    full_window_budget = 2 * ((3900.0 / CiSelfHostedAttempt::POLL_SECONDS).ceil * 3 + 1 + 5)
    assert_operator calls.length, :<=, full_window_budget
    assert_operator full_window_budget, :<=, 800
    calls.map { |call| call[:time] }.uniq.each do |start|
      count = calls.count { |call| call[:time] >= start && call[:time] < start + 3600 }
      assert_operator count, :<=, 800, "rolling hour at #{start} must leave at least 200 requests"
    end
  end

  def test_legal_git_branches_are_validated_as_argv_without_shell_interpretation
    original = Open3.method(:capture3)
    arguments = []
    validator = lambda do |*argv|
      arguments << argv
      original.call(*argv)
    end
    branches = ['feature/ci+fallback', '修复/CI兜底', '$(false)', 'feature/topic.locked', 'foo@bar']
    Open3.stub(:capture3, validator) do
      branches.each do |name|
        @api = FakeApi.new
        @api.scenario = ->(_time) { { run: { 'head_branch' => name } } }
        result = attempt(ref: "refs/heads/#{name}")
        assert result.passed, "#{name}: #{result.reason}"
        assert_equal name, @api.calls.first.dig(:body, 'ref')
      end
    end
    assert_equal branches.map { |name| ['git', 'check-ref-format', "refs/heads/#{name}"] }, arguments
  end

  def test_invalid_inputs_never_dispatch
    [{ sha: 'bad' }, { ref: 'refs/tags/v1' }, { ref: 'refs/heads/a b' }, { ref: 'refs/heads/a..b' },
     { ref: 'refs/heads/.hidden/topic' }, { ref: 'refs/heads/foo.lock/bar' }, { ref: 'refs/heads/a@{b' },
     { ref: 'refs/heads/a//b' }, { ref: 'refs/heads/a/' }, { ref: 'refs/heads/a.' }, { ref: 'refs/heads/' },
     { ref: "refs/heads/a\nb" }, { repo: '../other' }, { os: 'windows-latest' },
     { parent_id: 0 }, { parent_attempt: 'x' }, { nonce: 'x' }].each do |change|
      @api = FakeApi.new
      refute attempt(**change).passed, change.inspect
      assert_empty @api.calls
    end
  end
end

class CiRunnerWorkflowContractTest < Minitest::Test
  include CiRunnerContracts

  def setup
    @ci = YAML.load_file(File.join(ROOT, '.github/workflows/ci.yml'))
    @worker = YAML.load_file(File.join(ROOT, CiSelfHostedAttempt::WORKFLOW))
    @release = YAML.load_file(File.join(ROOT, '.github/workflows/release.yml'))
    @suite = YAML.load_file(File.join(ROOT, '.github/actions/ci-test/action.yml')).fetch('runs').fetch('steps')
  end

  def test_hosted_fallback_is_explicit_and_its_failure_is_a_real_gate
    job = @ci.fetch('jobs').fetch('test')
    assert_equal '${{ matrix.os }}', job.fetch('runs-on')
    assert_equal 135, job.fetch('timeout-minutes')
    refute job.key?('continue-on-error')
    fallback = job.fetch('steps').find { |step| step['name'] == 'Run complete hosted CI suite' }
    assert_equal "${{ !cancelled() && steps.self_hosted.outputs.self_hosted_passed != 'true' }}", fallback.fetch('if')
    assert_equal 60, fallback.fetch('timeout-minutes')
    refute fallback.key?('continue-on-error')
    assert_equal './.github/actions/ci-test', fallback.fetch('uses')
  end

  def test_pr_fork_tag_and_release_cannot_dispatch
    step = @ci.fetch('jobs').fetch('test').fetch('steps').find { |item| item['id'] == 'self_hosted' }
    assert_equal "github.event_name == 'push' && github.workflow == 'CI' && github.workflow_ref == format('{0}/.github/workflows/ci.yml@{1}', github.repository, github.ref) && !startsWith(github.ref, 'refs/tags/') && github.event.repository.fork == false", step.fetch('if')
    assert_equal 'ruby scripts/ci_self_hosted_attempt.rb', step.fetch('run')
    assert step.fetch('continue-on-error')
    assert_equal false, @ci.dig('jobs', 'test', 'steps', 0, 'with', 'persist-credentials')
    assert_equal({ 'contents' => 'read' }, @ci.fetch('permissions'))
    assert_equal 'write', @release.dig('jobs', 'verify', 'permissions', 'actions')
    assert_equal %w[linux macos verify], @release.dig('jobs', 'publish', 'needs')
  end

  def test_worker_dispatch_cannot_create_skipped_checks_with_original_gate_names
    assert_equal '.github/workflows/ci-self-hosted.yml', CiSelfHostedAttempt::WORKFLOW
    main_events = @ci.fetch('on') { @ci.fetch(true) }
    worker_events = @worker.fetch('on') { @worker.fetch(true) }
    assert_equal %w[pull_request push workflow_call], main_events.keys.sort
    assert_equal ['workflow_dispatch'], worker_events.keys
    # 有 PR 的分支只走 pull_request。push 仅保留默认分支与发版 tag，避免同一提交占两套名额。
    assert_equal %w[develop main], Array(main_events.dig('push', 'branches')).sort
    assert_equal ['v*'], Array(main_events.dig('push', 'tags'))
    assert_equal 'ci-${{ github.workflow }}-${{ github.event.pull_request.number || github.ref }}', @ci.dig('concurrency', 'group')
    assert_equal true, @ci.dig('concurrency', 'cancel-in-progress')
    assert_equal %w[linux-architectures test wsl-smoke], @ci.fetch('jobs').keys.sort
    assert_equal ['self-hosted-worker'], @worker.fetch('jobs').keys
    %w[test linux-architectures wsl-smoke].each do |name|
      refute @ci.fetch('jobs').fetch(name).key?('if'), "original #{name} check must remain unconditional"
    end
    assert_equal 'CI self-hosted attempt', @worker.fetch('name')
    refute_equal @ci.fetch('name'), @worker.fetch('name')
    worker_name = @worker.dig('jobs', 'self-hosted-worker', 'name')
    assert_equal 'CI self-hosted worker (${{ inputs.worker_os }})', worker_name
    final_names = CiSelfHostedAttempt::PLATFORMS.keys.map { |os| "test (#{os})" }
    final_names += @ci.dig('jobs', 'linux-architectures', 'strategy', 'matrix', 'include').map do |entry|
      "Linux #{entry.fetch('target')}"
    end
    final_names << @ci.dig('jobs', 'wsl-smoke', 'name')
    CiSelfHostedAttempt::PLATFORMS.each_key do |os|
      refute_includes final_names, worker_name.sub('${{ inputs.worker_os }}', os)
    end
  end

  def test_worker_cannot_recurse_and_retains_identity_and_complete_suite_evidence
    worker = @worker.fetch('jobs').fetch('self-hosted-worker')
    assert_includes worker.fetch('if'), "github.event_name == 'workflow_dispatch'"
    assert_includes worker.fetch('if'), "github.workflow == 'CI self-hosted attempt'"
    assert_includes worker.fetch('if'), 'github.event.repository.fork == false'
    CiSelfHostedAttempt::PLATFORMS.each_value do |labels|
      labels.each { |label| assert_includes worker.fetch('runs-on'), %Q("#{label}") }
    end
    assert_equal({ 'contents' => 'read' }, @worker.fetch('permissions'))
    assert_equal 'ci-worker-${{ inputs.request_id }}', @worker.fetch('run-name')
    assert_equal 60, worker.fetch('timeout-minutes')
    assert worker.fetch('continue-on-error')
    steps = worker.fetch('steps')
    assert_equal CiSelfHostedAttempt::REQUIRED_STEPS.drop(1), steps.map { |step| step['name'] }
    refute steps.last.key?('if')
    refute steps.any? { |step| step['continue-on-error'] }
    identity = steps.find { |step| step['name'] == 'Validate worker identity' }
    assert_equal '${{ inputs.request_id }}', identity.dig('env', 'REQUEST_ID')
    assert_includes identity.fetch('run'), 'ENV.fetch("GITHUB_SHA")'
    assert_includes identity.fetch('run'), 'refs/heads/'
    checkout = steps.find { |step| step['uses'] == 'actions/checkout@v4' }
    assert_equal '${{ inputs.expected_sha }}', checkout.dig('with', 'ref')
    assert_equal false, checkout.dig('with', 'persist-credentials')
    assert_equal './.github/actions/ci-test', steps.find { |step| step['name'] == 'Run complete CI suite' }.fetch('uses')
  end

  def test_all_original_main_tests_and_path_bases_are_preserved
    expected = ['Activate pinned Yarn 1', 'Build embedded Web frontend', 'Lint Web frontend',
                'Configuration Web bridge regression', 'Magpie Agent plugin contract', 'Format',
                'Hand-maintained file size boundary', 'Test', 'Clippy', 'Model eval task set self-check',
                'Agent metrics publish self-check', 'Prompt RSI gate self-check',
                'Eval concurrency, resume and feedback health', 'CI samples parse', 'SDK crates package',
                'npm package contract']
    assert_equal expected, @suite.filter_map { |step| step['name'] } - ['CI runner fallback offline contract']
    @suite.select { |step| step['run'] }.each { |step| assert_equal 'bash', step.fetch('shell') }
    assert_equal 'web', @suite.find { |step| step['name'] == 'Build embedded Web frontend' }.fetch('working-directory')
    assert_equal 'npm/willdeep', @suite.find { |step| step['name'] == 'npm package contract' }.fetch('working-directory')
    contract = @suite.find { |step| step['name'] == 'CI runner fallback offline contract' }
    assert_equal "runner.os == 'Linux'", contract.fetch('if')
    assert_includes contract.fetch('run'), 'scripts/test/ci_self_hosted_attempt_test.rb'
  end
end
