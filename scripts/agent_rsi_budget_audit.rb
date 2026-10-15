#!/usr/bin/env ruby
# frozen_string_literal: true
require 'optparse'
require 'bigdecimal'
require_relative 'lib/agent_rsi_format_replay'
require_relative 'lib/agent_rsi_budget_policy'

module AgentRsiBudgetAudit
  module_function
  def require!(condition, message) = (raise AgentRsiFormatReplay::Denied, message unless condition)
  def read(path) = JSON.parse(File.binread(path))
  def digest(value) = ContractGolden.hash(value)
  def percentile(values, fraction) = values.sort[[(values.length * fraction).ceil - 1, 0].max]

  def response_binding(record, response, ordinal, manifest, diagnostic: nil, reconciliation: nil)
    provider = JSON.parse(response.fetch('body'))
    require!(response['http_status'] == 200 && response.dig('headers', 'x-request-id') == record['request_id'] &&
      response.dig('headers', 'x-routed-model') == manifest['expected_routed_model'] &&
      response.dig('headers', 'x-app-version') == manifest['expected_gateway_version'] &&
      provider['model'] == manifest['expected_response_model'], 'saved response identity or route changed')
    require!(provider.fetch('usage').values_at('prompt_tokens', 'completion_tokens', 'total_tokens') ==
      record.values_at('input_tokens', 'output_tokens', 'actual_tokens'), 'saved provider usage changed')
    return 'original_response_digest' if record.key?('response_sha256') && digest(provider) == record['response_sha256']
    # Only the first historical pricing probe lacks a response hash. Apply the
    # same separate receipt binding as its passing frozen audit, never a general
    # missing-hash exemption, and label this narrower proof in the derived facts.
    require!(ordinal == 1 && !record.key?('response_sha256') && !record.key?('actual_pricing_snapshot') &&
      diagnostic && diagnostic['request_id'] == record['request_id'], 'saved provider response digest unavailable or changed')
    bill = record.fetch('billing_receipt')
    cost = BigDecimal(bill.fetch('cost'))
    require!(cost.finite? && cost >= 0 && bill['request_id'] == record['request_id'] &&
      (cost * 1_000_000).ceil == record['verified_bill_microusd'] && reconciliation &&
      reconciliation.any? { |r| r['request_id'] == record['request_id'] && BigDecimal(r.fetch('cost')) == cost },
      'historical first response reconciliation changed')
    'legacy_first_response_identity_usage_and_separate_billing_receipts'
  end

  def run(batch, audit_path, output)
    batch = File.realpath(batch)
    output = File.join(File.realpath(File.dirname(output)), File.basename(output))
    require!(!File.exist?(output) && !File.symlink?(output) && !output.start_with?(batch + '/'), 'output must be new and outside saved batch')
    before = AgentRsiFormatReplay.inventory(batch)
    audit = read(audit_path)
    state = read(File.join(batch, 'ledger.json'))
    manifest = read(File.join(batch, 'manifest.json'))
    require!(audit['passed'] == true && audit['mode'] == 'final' && audit['issues'] == [] && audit['known_planned_terminals'] == 288,
      'passing original final audit required')
    require!(Digest::SHA256.file(File.join(batch, 'ledger.json')).hexdigest == audit['ledger_raw_sha256'] &&
      digest(manifest) == audit['manifest_sha256'] && state['manifest_sha256'] == audit['manifest_sha256'], 'audit/input binding changed')
    require!(state['paused'].nil? && AgentRsiBudgetPolicy.usage(state['requests'])['billing_complete'], 'unresolved usage or billing')
    attempts = state.fetch('attempts').to_h { |a| [a.fetch('trial_id'), a] }
    facts, global_used, trial_used = [], 0, Hash.new(0)
    state.fetch('requests').each_with_index do |record, i|
      body = read(File.join(batch, format('request-%04d.json', i + 1)))
      require!(digest(body) == record['request_sha256'], 'saved physical request changed')
      body['max_tokens'] = AgentRsiBudgetPolicy::OUTPUT_LIMIT
      require!(digest(body) == record['requested_body_sha256'], 'original requested output limit not reconstructible')
      decision = AgentRsiBudgetPolicy.reservation(body, trial_used: trial_used[record['trial_id']], global_used: global_used)
      require!(decision['admissible_by_token_policy'] && decision['input_bound_tokens'] == record['reserved_input_tokens'] &&
        decision['admitted_output_tokens'] == record['max_tokens'] &&
        record['reserved_tokens'] == record['reserved_input_tokens'] + record['max_tokens'], 'saved admission differs from frozen byte-bound policy')
      require!(record['input_tokens'] <= record['reserved_input_tokens'] && record['output_tokens'] <= record['max_tokens'], 'actual tokens exceed original reservation')
      response = read(File.join(batch, format('response-%04d.json', i + 1)))
      provider = JSON.parse(response.fetch('body'))
      binding = response_binding(record, response, i + 1, manifest,
        diagnostic: i.zero? ? read(File.join(batch, 'first-request-pricing-diagnostic.json')) : nil,
        reconciliation: i.zero? ? read(File.join(batch, 'first-request-billing-reconciliation.json')).fetch('rows') : nil)
      owner = attempts.fetch(record['trial_id'])
      facts << {'physical_request_ordinal' => i + 1, 'platform' => owner['platform'], 'arm' => owner['arm'],
        'original_epoch' => owner.fetch('scoring_protocol', 'v1-historical'),
        'reserved_input_tokens' => record['reserved_input_tokens'], 'actual_input_tokens' => record['input_tokens'],
        'reserved_minus_actual_input' => record['reserved_input_tokens'] - record['input_tokens'],
        'reserved_output_tokens' => record['max_tokens'], 'actual_output_tokens' => record['output_tokens'],
        'output_cap_was_clamped' => record['max_tokens'] < AgentRsiBudgetPolicy::OUTPUT_LIMIT,
        'finish_reason' => provider.dig('choices', 0, 'finish_reason'),
        'response_binding' => binding, 'derived_response_sha256' => digest(provider),
        'actual_trial_tokens_before_request' => trial_used[record['trial_id']]}
      global_used += record['actual_tokens']
      trial_used[record['trial_id']] += record['actual_tokens']
    end
    budgets = state['attempts'].select { |a| a['status'] == 'budget_exhausted' }.map do |a|
      observation = read(File.join(batch, a['trial_id'], 'observation.json'))
      physical = state['requests'].select { |r| r['trial_id'] == a['trial_id'] }
      native = observation.fetch('request_receipts')
      extra = native.length == physical.length + 1 ? native.last : nil
      denied = extra && extra['status'] == 403 && extra['transport_valid'] == false &&
        %w[receipt_id request_id response_id routed_model usage].all? { |k| extra.key?(k) && extra[k].nil? }
      actual = physical.sum { |r| r['actual_tokens'] }
      # This is the frozen Rust serializer proof used by the original audit;
      # the denied fourth body was NOT captured by the host. Do not fabricate it.
      fixed_envelope_delta = 947
      native_input_bound = denied && extra['request_utf8_bytes'].is_a?(Integer) ? extra['request_utf8_bytes'] + fixed_envelope_delta : nil
      a.slice('platform', 'arm', 'split').merge('physical_request_count' => physical.length,
        'actual_tokens_at_stop' => actual, 'did_truncate' => observation['did_truncate'],
        'native_worker_status' => observation['worker_status'], 'original_audited_terminal' => 'budget_exhausted',
        'unadmitted_local_receipt_present' => !!denied, 'denied_body_captured_by_host' => false,
        'frozen_native_next_input_bound' => native_input_bound,
        'native_byte_count_proves_next_admission_exhausted' => native_input_bound ? actual + native_input_bound >= AgentRsiBudgetPolicy::TRIAL_TOKEN_LIMIT : nil,
        'claim_limit' => 'Original audited terminal preserved. Native byte count is distinct from a captured denied request or actual token use.')
    end
    groups = facts.group_by { |f| f['platform'] }.map do |platform, requests|
      ratios = requests.filter_map { |r| r['reserved_input_tokens'].to_f / r['actual_input_tokens'] if r['actual_input_tokens'].positive? }
      {'platform' => platform, 'physical_requests' => requests.length,
       'output_clamped_requests' => requests.count { |r| r['output_cap_was_clamped'] },
       'input_reserved_total' => requests.sum { |r| r['reserved_input_tokens'] },
       'actual_input_total' => requests.sum { |r| r['actual_input_tokens'] },
       'median_reserved_to_actual_input_ratio' => percentile(ratios, 0.5),
       'p95_reserved_to_actual_input_ratio' => percentile(ratios, 0.95)}
    end
    require!(before == AgentRsiFormatReplay.inventory(batch), 'original batch changed during diagnostic')
    result = {'schema' => 'willdeep.agent-rsi.saved-budget-audit.v1', 'physical_requests_verified' => facts.length,
      'reservation_formula_matches_every_saved_request' => true, 'token_caps_relaxed' => false,
      'legacy_first_response_hash_absent_in_original_ledger' => facts.first['response_binding'].start_with?('legacy_first'),
      'byte_bound_policy' => 'JSON UTF-8 bytes(messages, tools, tool_choice) + 1024; not actual tokenizer counts',
      'budget_terminals' => budgets, 'platform_summaries' => groups,
      'original_batch_tree_sha256' => digest(before), 'original_batch_entries_unchanged' => before.length,
      'original_ledger_raw_sha256' => audit['ledger_raw_sha256'], 'original_results_modified' => false,
      'new_model_requests' => 0, 'new_model_cost_microusd' => 0,
      'verified_batch_cost_microusd_unchanged' => state['requests'].sum { |r| r['verified_bill_microusd'] },
      'policy_file_sha256' => Digest::SHA256.file(File.join(__dir__, 'lib/agent_rsi_budget_policy.rb')).hexdigest,
      'analysis_file_sha256' => Digest::SHA256.file(__FILE__).hexdigest,
      'claim_limit' => 'Offline diagnosis of a conservative bound; no model/tokenizer calls, no changed budgets, no quality score, no proof of a replacement tokenizer.'}
    Dir.mkdir(output, 0o700)
    File.write(File.join(output, 'budget-audit.json'), JSON.pretty_generate(result) + "\n", mode: 'wx', perm: 0o600)
    File.write(File.join(output, 'request-facts.json'), JSON.pretty_generate(facts) + "\n", mode: 'wx', perm: 0o600)
    File.write(File.join(output, 'budget-audit.md'), markdown(result), mode: 'wx', perm: 0o600)
    result
  end

  def markdown(r)
    lines = ['# Rust / Mac 保存请求的免费预算诊断', '',
      "逐笔核对 #{r['physical_requests_verified']} 个保存物理请求，全部与冻结预留公式相同；未放宽任何 cap。新增模型请求及费用均为 0。", '',
      '输入预留为 messages/tools/tool_choice 的 JSON UTF-8 字节数加 1024，属于保守上界，不是实际分词量。历史和工具 schema 随请求重复纳入；实际 token 未满仍可能没有足够空间准入下一请求。', '',
      '| 平台 | 物理请求 | 输出 cap 被压低的请求 | 输入预留/实际输入 中位数 | P95 |', '|---|---:|---:|---:|---:|']
    r['platform_summaries'].each { |g| lines << "| #{g['platform']} | #{g['physical_requests']} | #{g['output_clamped_requests']} | #{format('%.2f', g['median_reserved_to_actual_input_ratio'])} | #{format('%.2f', g['p95_reserved_to_actual_input_ratio'])} |" }
    lines += ['', "原 #{r['budget_terminals'].length} 个预算终态单列，原评分与账本不变。免费本地拒绝没有物理请求、Token 或新增费用；已经发出的未知用量或账单仍必须保持未知并停止。", '',
      '下一步应验证真实分词估计或历史尺寸控制，再另行冻结资源规则；本次不以估算替换上界，不提高 4 请求/16000 token 原上限。', '']
    lines.join("\n")
  end
end

if $PROGRAM_NAME == __FILE__
  options = {}
  OptionParser.new do |p|
    p.on('--batch PATH') { |v| options[:batch] = v }
    p.on('--audit PATH') { |v| options[:audit] = v }
    p.on('--output PATH') { |v| options[:output] = v }
  end.parse!
  begin
    r = AgentRsiBudgetAudit.run(options.fetch(:batch), options.fetch(:audit), options.fetch(:output))
    puts JSON.generate(r.slice('physical_requests_verified', 'reservation_formula_matches_every_saved_request', 'new_model_requests', 'original_results_modified', 'platform_summaries'))
  rescue AgentRsiFormatReplay::Denied, AgentRsiBudgetPolicy::Invalid, KeyError, Errno::ENOENT => error
    warn "Offline budget audit rejected: #{error.class}: #{error.message}"
    exit 2
  end
end
