# frozen_string_literal: true
require 'json'
require 'digest'

# Pure diagnostics for the existing byte-bound admission policy. This does not
# grant execution, enlarge any cap, initialize an Account, or dispatch a request.
module AgentRsiBudgetPolicy
  FRAMING_ALLOWANCE = 1024
  OUTPUT_LIMIT = 4000
  TRIAL_TOKEN_LIMIT = 16_000
  GLOBAL_TOKEN_LIMIT = 5_760_000
  class Invalid < StandardError; end
  module_function

  def nonnegative!(value)
    raise Invalid, 'expected a nonnegative integer' unless value.is_a?(Integer) && value >= 0
    value
  end

  def reservation(body, trial_used:, global_used:, trial_limit: TRIAL_TOKEN_LIMIT, global_limit: GLOBAL_TOKEN_LIMIT)
    [trial_used, global_used, trial_limit, global_limit].each { |v| nonnegative!(v) }
    raise Invalid, 'missing messages or output limit' unless body.is_a?(Hash) && body['messages'].is_a?(Array)
    requested = Integer(body.fetch('max_tokens'))
    raise Invalid, 'output limit must be positive' unless requested.positive?
    bytes = JSON.generate(body.slice('messages', 'tools', 'tool_choice')).bytesize
    bound = bytes + FRAMING_ALLOWANCE
    trial_headroom = trial_limit - trial_used - bound
    global_headroom = global_limit - global_used - bound
    cap = [requested, OUTPUT_LIMIT, trial_headroom, global_headroom].min
    reason = if trial_headroom <= 0 then 'trial_input_reservation_exhausted'
             elsif global_headroom <= 0 then 'global_input_reservation_exhausted'
             end
    {'schema' => 'willdeep.agent-rsi.token-reservation-diagnostic.v1',
     'request_utf8_sha256' => Digest::SHA256.hexdigest(JSON.generate(body)),
     'payload_utf8_bytes' => bytes, 'framing_allowance' => FRAMING_ALLOWANCE,
     'input_bound_tokens' => bound, 'physical_trial_tokens_used' => trial_used,
     'physical_global_tokens_used' => global_used, 'trial_token_limit' => trial_limit, 'global_token_limit' => global_limit,
     'requested_output_tokens' => requested, 'admitted_output_tokens' => [cap, 0].max,
     'admissible_by_token_policy' => cap.positive?, 'reason' => reason,
     'physical_request_created' => false, 'model_requests' => 0,
     'claim_limit' => 'Conservative UTF-8 byte bound, not a tokenizer estimate or proof that actual tokens reached the limit.'}
  rescue KeyError, TypeError, ArgumentError
    raise Invalid, 'invalid token admission input'
  end

  def usage(rows)
    raise Invalid, 'physical receipts must be an array' unless rows.is_a?(Array)
    known = rows.all? do |r|
      r['status'] == 'settled' && %w[input_tokens output_tokens actual_tokens].all? { |k| r[k].is_a?(Integer) && r[k] >= 0 } &&
        r['input_tokens'] + r['output_tokens'] == r['actual_tokens']
    end
    billed = known && rows.all? { |r| r['verified_bill_microusd'].is_a?(Integer) && r['verified_bill_microusd'] >= 0 }
    {'usage_complete' => known, 'billing_complete' => billed,
     'physical_request_count' => rows.length,
     'physical_tokens' => known ? rows.sum { |r| r['actual_tokens'] } : nil,
     'verified_bill_microusd' => billed ? rows.sum { |r| r['verified_bill_microusd'] } : nil}
  end

  def stop(rows, cause:, local_reservation: nil)
    facts = usage(rows)
    kind = if !facts['usage_complete'] then 'usage_unknown'
           elsif !facts['billing_complete'] then 'billing_unreconciled'
           elsif cause == 'local_token_reservation'
             raise Invalid, 'local denial lacks a known non-dispatched admission decision' unless local_reservation &&
               local_reservation['schema'] == 'willdeep.agent-rsi.token-reservation-diagnostic.v1' &&
               local_reservation['admissible_by_token_policy'] == false && local_reservation['physical_request_created'] == false &&
               %w[trial_input_reservation_exhausted global_input_reservation_exhausted].include?(local_reservation['reason'])
             'budget_exhausted'
           elsif %w[request_limit output_truncated].include?(cause) then 'budget_exhausted'
           elsif %w[timeout cancelled completed].include?(cause) then cause
           else raise Invalid, 'unknown stop cause'
           end
    facts.merge('schema' => 'willdeep.agent-rsi.free-stop-diagnostic.v1', 'stop_kind' => kind,
      'cause' => cause, 'unadmitted_local_denials' => kind == 'budget_exhausted' && cause == 'local_token_reservation' ? 1 : 0,
      'automatic_retry_allowed' => false, 'new_model_requests' => 0, 'limits_relaxed' => false)
  end
end
