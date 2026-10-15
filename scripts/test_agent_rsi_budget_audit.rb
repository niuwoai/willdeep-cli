#!/usr/bin/env ruby
# frozen_string_literal: true
require 'minitest/autorun'
require_relative 'agent_rsi_budget_audit'
require_relative 'lib/agent_rsi_free_test_report'
AgentRsiFreeTestReport.enable('budget-audit')

class AgentRsiBudgetAuditTest < Minitest::Test
  def setup
    @manifest = {'expected_routed_model' => 'mock-route', 'expected_response_model' => 'mock-model', 'expected_gateway_version' => 'mock-version'}
    @provider = {'model' => 'mock-model', 'usage' => {'prompt_tokens' => 10, 'completion_tokens' => 2, 'total_tokens' => 12}}
    @response = {'http_status' => 200, 'headers' => {'x-request-id' => 'fixture-id', 'x-routed-model' => 'mock-route',
      'x-app-version' => 'mock-version'}, 'body' => JSON.generate(@provider)}
    @record = {'request_id' => 'fixture-id', 'input_tokens' => 10, 'output_tokens' => 2, 'actual_tokens' => 12,
      'verified_bill_microusd' => 1, 'billing_receipt' => {'request_id' => 'fixture-id', 'cost' => '0.000001'}}
    @options = {diagnostic: {'request_id' => 'fixture-id'}, reconciliation: [{'request_id' => 'fixture-id', 'cost' => '0.000001'}]}
  end

  def bind(ordinal = 1, **options)
    AgentRsiBudgetAudit.response_binding(@record, @response, ordinal, @manifest, **options)
  end

  def test_normal_response_requires_exact_digest
    @record['response_sha256'] = ContractGolden.hash(@provider)
    assert_equal 'original_response_digest', bind(2)
    @record['response_sha256'] = '0' * 64
    assert_raises(AgentRsiFormatReplay::Denied) { bind(2) }
  end

  def test_first_historical_probe_is_explicitly_labelled
    assert_equal 'legacy_first_response_identity_usage_and_separate_billing_receipts', bind(**@options)
  end

  def test_no_general_missing_hash_exemption
    assert_raises(AgentRsiFormatReplay::Denied) { bind(2, **@options) }
  end

  def test_first_present_but_wrong_hash_cannot_use_legacy_fallback
    @record['response_sha256'] = '0' * 64
    assert_raises(AgentRsiFormatReplay::Denied) { bind(**@options) }
  end

  def test_newer_pricing_row_cannot_use_legacy_fallback
    @record['actual_pricing_snapshot'] = {}
    assert_raises(AgentRsiFormatReplay::Denied) { bind(**@options) }
  end

  def test_route_binding_is_required_for_historical_probe
    @response['headers']['x-routed-model'] = 'different'
    assert_raises(AgentRsiFormatReplay::Denied) { bind(**@options) }
  end

  def test_usage_binding_is_required_for_historical_probe
    @provider['usage']['total_tokens'] = 13
    @response['body'] = JSON.generate(@provider)
    assert_raises(AgentRsiFormatReplay::Denied) { bind(**@options) }
  end

  def test_missing_reconciliation_cannot_use_legacy_fallback
    assert_raises(AgentRsiFormatReplay::Denied) { bind(diagnostic: @options[:diagnostic]) }
  end

  def test_mismatched_billing_cannot_use_legacy_fallback
    @record['verified_bill_microusd'] = 2
    assert_raises(AgentRsiFormatReplay::Denied) { bind(**@options) }
  end

  def test_mismatched_pricing_diagnostic_cannot_use_legacy_fallback
    @options[:diagnostic]['request_id'] = 'different'
    assert_raises(AgentRsiFormatReplay::Denied) { bind(**@options) }
  end
end
