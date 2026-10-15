#!/usr/bin/env ruby
# frozen_string_literal: true
require 'minitest/autorun'
require_relative 'lib/agent_rsi_budget_policy'
require_relative 'lib/agent_rsi_free_test_report'
AgentRsiFreeTestReport.enable('budget-policy')

class AgentRsiBudgetPolicyTest < Minitest::Test
  def body(text = 'read fixture')
    {'model' => 'fixed-mock', 'messages' => [{'role' => 'user', 'content' => text}],
     'tools' => [], 'tool_choice' => 'auto', 'max_tokens' => 4000}
  end
  def receipt
    {'status' => 'settled', 'input_tokens' => 100, 'output_tokens' => 50, 'actual_tokens' => 150, 'verified_bill_microusd' => 70}
  end
  def reserve(input = body, **options)
    AgentRsiBudgetPolicy.reservation(input, trial_used: options.fetch(:trial_used, 0), global_used: options.fetch(:global_used, 0), **options.reject { |k, _| %i[trial_used global_used].include?(k) })
  end

  def test_bound_matches_current_ledger_formula_without_relaxation
    r = reserve
    assert_equal JSON.generate(body.slice('messages', 'tools', 'tool_choice')).bytesize + 1024, r['input_bound_tokens']
    assert_equal 4000, r['admitted_output_tokens']
    refute r['physical_request_created']
  end

  def test_utf8_bytes_are_not_character_count
    ascii = reserve(body('a'))
    unicode = reserve(body('界'))
    assert_equal 2, unicode['input_bound_tokens'] - ascii['input_bound_tokens']
  end

  def test_tool_schema_counts_in_reservation
    modified = body.merge('tools' => [{'function' => {'description' => 'x' * 400}}])
    assert_operator reserve(modified)['input_bound_tokens'], :>, reserve['input_bound_tokens']
  end

  def test_long_history_can_prevent_admission_before_actual_tokens_reach_cap
    r = reserve(body('x' * 12_000), trial_used: 5000)
    refute r['admissible_by_token_policy']
    assert_equal 'trial_input_reservation_exhausted', r['reason']
    assert_equal 0, r['admitted_output_tokens']
    assert_equal 5000, r['physical_trial_tokens_used']
  end

  def test_output_cap_shrinks_at_boundary
    r = reserve
    cap = r['input_bound_tokens'] + 37
    r = reserve(trial_limit: cap)
    assert r['admissible_by_token_policy']
    assert_equal 37, r['admitted_output_tokens']
  end

  def test_global_limit_also_prevents_request
    r = reserve(global_used: 5_760_000)
    refute r['admissible_by_token_policy']
    assert_equal 'global_input_reservation_exhausted', r['reason']
  end

  def test_local_denial_does_not_create_unknown_physical_usage
    r = reserve(body('x' * 17_000))
    stop = AgentRsiBudgetPolicy.stop([receipt], cause: 'local_token_reservation', local_reservation: r)
    assert_equal 'budget_exhausted', stop['stop_kind']
    assert stop['usage_complete']
    assert_equal 1, stop['physical_request_count']
    assert_equal 150, stop['physical_tokens']
    assert_equal 70, stop['verified_bill_microusd']
    assert_equal 1, stop['unadmitted_local_denials']
    refute stop['automatic_retry_allowed']
  end

  def test_real_unknown_usage_overrides_local_budget_story
    r = reserve(body('x' * 17_000))
    stop = AgentRsiBudgetPolicy.stop([receipt.merge('status' => 'unknown', 'actual_tokens' => nil)], cause: 'local_token_reservation', local_reservation: r)
    assert_equal 'usage_unknown', stop['stop_kind']
    assert_nil stop['physical_tokens']
    assert_nil stop['verified_bill_microusd']
    assert_equal 0, stop['unadmitted_local_denials']
  end

  def test_unknown_bill_stays_unresolved_even_with_known_tokens
    stop = AgentRsiBudgetPolicy.stop([receipt.reject { |k, _| k == 'verified_bill_microusd' }], cause: 'request_limit')
    assert_equal 'billing_unreconciled', stop['stop_kind']
    assert_nil stop['verified_bill_microusd']
  end

  %w[timeout cancelled completed request_limit output_truncated].each do |cause|
    define_method('test_stop_' + cause) do
      stop = AgentRsiBudgetPolicy.stop([receipt], cause: cause)
      assert_equal 1, stop['physical_request_count']
      assert_equal 0, stop['new_model_requests']
      refute stop['limits_relaxed']
    end
  end

  def test_admitted_request_cannot_masquerade_as_local_denial
    assert_raises(AgentRsiBudgetPolicy::Invalid) { AgentRsiBudgetPolicy.stop([receipt], cause: 'local_token_reservation', local_reservation: reserve) }
  end

  def test_invalid_usage_and_limits_are_not_zero_filled
    assert_raises(AgentRsiBudgetPolicy::Invalid) { reserve(trial_used: -1) }
    assert_raises(AgentRsiBudgetPolicy::Invalid) { reserve(body.merge('max_tokens' => 0)) }
    assert_equal 'usage_unknown', AgentRsiBudgetPolicy.stop([receipt.merge('actual_tokens' => 999)], cause: 'completed')['stop_kind']
  end

  def test_pure_policy_does_not_load_paid_runtime
    refute $LOADED_FEATURES.any? { |p| p.match?(/net\/http|agent_rsi_live(?:_account|_host)?\.rb/) }
  end
end
