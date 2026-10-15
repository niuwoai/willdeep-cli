# frozen_string_literal: true
require 'json'
require 'digest'
require 'open3'
require 'optparse'
require 'time'
require 'fileutils'
require_relative 'lib/agent_rsi_reproduction'
require_relative 'lib/agent_rsi_contract'

# Read existing native mock artifacts; never runs a model or changes observations.
module AgentRsiReproductionNativeAcceptance
  module_function
  def require!(condition, code)
    raise AgentRsiReproduction::Denied, code unless condition
  end

  def mac_case(root, task, output, baseline, contract)
    directory = File.join(root, task.fetch('case_id'))
    paths = %w[observation.json raw_model_content.json host_evidence.json first_request.json events.json].to_h { |name| [name, File.join(directory, name)] }
    before = paths.transform_values { |path| Digest::SHA256.file(path).hexdigest }
    observation = JSON.parse(File.read(paths['observation.json']))
    raw = File.read(paths['raw_model_content.json'], encoding: Encoding::UTF_8)
    evidence = JSON.parse(File.read(paths['host_evidence.json']))
    request = JSON.parse(File.read(paths['first_request.json']))
    events = JSON.parse(File.read(paths['events.json']))
    checks = {}
    checks['case_identity'] = observation['case_id'] == task['case_id']
    checks['native_unverified_retained'] = observation['status'] == 'unverified' && observation['host_reproduction_verdict'] == 'pending_shared_host_grader'
    checks['offline_only'] = observation['execution_mode'] == 'offline_mock_only' && observation['model_calls_authorized'] == 0
    checks['original_baseline_revision'] = observation['definition_revision'] == baseline['sha256'] && observation['loaded_revision'] == baseline['sha256'] && ContractGolden.hash(baseline['definition']) == baseline['sha256']
    checks['original_baseline_instructions'] = observation['definition_instructions_observed'] == true && observation['definition_instructions_sha256'] == Digest::SHA256.hexdigest(baseline['definition']['instructions']) &&
      request.fetch('messages').any? { |message| message['content'].to_s.include?(baseline['definition']['instructions']) }
    checks['shared_contract_bytes'] = observation['contract_sha256'] == Digest::SHA256.file(File.join(AgentRsiReproduction::ROOT, 'bench/agent-rsi/review-output-v2/contract.json')).hexdigest
    checks['shared_contract_instructions'] = observation['instructions_observed'] == true && observation['instructions_sha256'] == Digest::SHA256.hexdigest(contract.fetch('instructions')) &&
      request['messages'].any? { |message| message['content'].to_s.include?(contract['instructions']) }
    checks['raw_answer_identical'] = raw == observation['raw_model_content']
    checks['host_evidence_identical'] = evidence == observation['host_evidence'] && evidence['fixture_sha256'] == task['fixture_sha256'] &&
      %w[read_observed delivered_to_model fixture_unchanged].all? { |name| evidence[name] == true }
    checks['first_request_hash'] = observation['first_request_sha256'] == before['first_request.json']
    checks['events_hash'] = observation['events_sha256'] == before['events.json']
    worker_started = events.find { |event| event['kind'] == 'started' && event.dig('definition', 'revision') == baseline['sha256'] }
    checks['root_worker_lineage'] = worker_started && worker_started['rootRunID'] != worker_started['runID'] && worker_started['parentRunID'] == worker_started['rootRunID']
    checks['successful_native_read_receipt'] = worker_started && events.any? { |event| event['runID'] == worker_started['runID'] && event['kind'] == 'tool_receipt' && event['toolName'] == 'read_file' && event['toolSucceeded'] == true }
    require!(checks.values.all?, 'native_binding_failed:' + checks.select { |_, value| !value }.keys.join(','))
    prefix = File.join(output, 'mac-' + task['case_id'])
    stdout, status = Open3.capture2e('ruby', File.join(AgentRsiReproduction::ROOT, 'scripts/lib/agent_rsi_reproduction_eval.rb'),
      '--task', task['case_id'], '--report', paths['raw_model_content.json'], '--evidence', paths['host_evidence.json'], '--output-prefix', prefix)
    require!(status.success?, 'host_cli_failed')
    verdict = JSON.parse(File.read(prefix + '.json'))
    require!(verdict['passed'] == true && verdict['report_sha256'] == before['raw_model_content.json'], 'host_verification_failed')
    require!(before == paths.transform_values { |path| Digest::SHA256.file(path).hexdigest }, 'original_native_artifact_changed')
    {'platform' => 'macos-native-worker', 'case_id' => task['case_id'], 'checks' => checks,
     'native_status_retained' => observation['status'], 'native_artifact_sha256' => before,
     'original_baseline_revision' => baseline['sha256'], 'contract_sha256' => observation['contract_sha256'],
     'host_verification' => verdict, 'host_cli_summary' => JSON.parse(stdout)}
  end

  def rust_prompt_checks(observation, prompt, baseline, contract)
    context = observation.fetch('worker_context')
    receipts = observation.fetch('mock_receipts')
    actual_sha = Digest::SHA256.hexdigest(prompt)
    marker = 'Output instructions SHA-256: ' + observation['instructions_sha256']
    marker_index = prompt.index(marker)
    projection = observation['request_context_projection']
    {
      'first_admitted_system_prompt_bytes' => actual_sha == observation['effective_prompt_sha256'] &&
        receipts.first['system_prompt'] == prompt && receipts.first['system_prompt_sha256'] == actual_sha,
      'every_admitted_system_prompt_independently_bound' => receipts.all? do |receipt|
        receipt_prompt = receipt['system_prompt']
        receipt_prompt.is_a?(String) && Digest::SHA256.hexdigest(receipt_prompt) == receipt['system_prompt_sha256']
      end,
      'every_prompt_preserves_locked_prelude_and_instructions' => receipts.all? do |receipt|
        receipt_prompt = receipt['system_prompt']
        next false unless receipt_prompt.is_a?(String)
        receipt_marker = receipt_prompt.index(marker)
        receipt_marker && receipt_prompt.include?(baseline['definition']['instructions']) && receipt_prompt.include?(contract['instructions']) &&
          Digest::SHA256.hexdigest(receipt_prompt[0...(receipt_marker + marker.length)]) == context['effective_prompt_sha256']
      end,
      'original_baseline_and_output_instructions_present' => prompt.include?(baseline['definition']['instructions']) && prompt.include?(contract['instructions']),
      'compiled_prelude_bound_without_relabeling' => marker_index && context['effective_prompt_sha256'] == Digest::SHA256.hexdigest(prompt[0...(marker_index + marker.length)]),
      'derived_projection_explicit_and_exact' => projection.is_a?(Hash) && projection['derived'] == true && projection['source'] == 'first_admitted_mock_request' &&
        projection['context'] == context.merge('effective_prompt_sha256' => actual_sha)
    }
  end

  def rust_bindings(directory, task, baseline, contract)
    observation_path = File.join(directory, 'observation.json')
    observation = JSON.parse(File.read(observation_path))
    receipts = observation.fetch('mock_receipts')
    evidence = JSON.parse(File.read(File.join(directory, 'evidence.json')))
    context = observation.fetch('worker_context')
    projection = observation['request_context_projection']
    lifecycle_paths = Dir[File.join(directory, 'home/agent-evidence/*.json')]
    lifecycle = lifecycle_paths.map do |path|
      require!(File.basename(path, '.json') == Digest::SHA256.file(path).hexdigest, 'original_worker_event_file_digest')
      JSON.parse(File.read(path))
    end
    checks = {
      'case_identity' => observation['case_id'] == task['case_id'],
      'native_unverified_retained' => observation['verification'] == 'unverified' && observation.key?('objective_score') && observation['objective_score'].nil? && observation['quality_claim'] == false,
      'offline_only' => observation['execution_mode'] == 'offline_mock_only' && observation['model_calls_authorized'] == 0 && observation['network_model_calls'] == 0,
      'original_baseline_revision' => observation['definition_revision'] == baseline['sha256'] && context['definition_revision'] == baseline['sha256'] && ContractGolden.hash(baseline['definition']) == baseline['sha256'],
      'shared_contract_bytes' => observation['contract_sha256'] == Digest::SHA256.file(File.join(AgentRsiReproduction::ROOT, 'bench/agent-rsi/review-output-v2/contract.json')).hexdigest,
      'shared_contract_instructions_sha' => observation['instructions_sha256'] == Digest::SHA256.hexdigest(contract['instructions']),
      'original_worker_context_retained' => observation['compiled_worker_prompt_sha256'] == context['effective_prompt_sha256'] && lifecycle.any? { |event| event['phase'] == 'started' && event['context'] == context } && lifecycle.all? { |event| event['context'] == context },
      'original_worker_lifecycle_retained' => lifecycle.length == observation.fetch('worker_lifecycle').length && lifecycle.all? { |event| observation['worker_lifecycle'].include?(event) },
      'root_worker_lineage' => context['root_run_id'].is_a?(String) && !context['root_run_id'].empty? && context['worker_run_id'].is_a?(String) && context['worker_run_id'] != context['root_run_id'],
      'host_evidence_identical' => evidence == observation['evidence'] && evidence['fixture_sha256'] == task['fixture_sha256'],
      'receipt_count_exact' => receipts.length == observation['mock_requests_admitted'] && receipts.length <= observation.dig('limits', 'max_requests'),
      'only_synthetic_mock_receipts' => receipts.all? { |receipt| receipt['mode'] == 'offline_mock_only' && receipt['usage_kind'] == 'synthetic' }
    }
    paths = [observation_path, File.join(directory, 'evidence.json')] + lifecycle_paths
    if receipts.empty?
      checks['no_request_does_not_invent_prompt_projection'] = projection.nil? && observation['effective_prompt_sha256'].nil? && !File.exist?(File.join(directory, 'effective-prompt.txt'))
    else
      prompt_path = File.join(directory, 'effective-prompt.txt')
      prompt = File.read(prompt_path, encoding: Encoding::UTF_8)
      paths << prompt_path
      checks.merge!(rust_prompt_checks(observation, prompt, baseline, contract))
    end
    require!(checks.values.all?, 'rust_native_binding_failed:' + checks.select { |_, value| !value }.keys.join(','))
    [observation, evidence, checks, paths]
  end

  def rust_case(directory, task, output, baseline, contract)
    # Snapshot before opening any host result; original Worker context is never
    # rewritten into the derived first-request projection by this acceptance.
    existing_paths = Dir[File.join(directory, '**', '*')].select { |path| File.file?(path) }
    before = existing_paths.to_h { |path| [path, Digest::SHA256.file(path).hexdigest] }
    observation, evidence, checks, = rust_bindings(directory, task, baseline, contract)
    limits = observation.fetch('limits')
    ordinary_case = limits.values_at('max_requests', 'max_tokens', 'read_rounds', 'timeout_ms', 'cancel_after_ms', 'mock_delay_ms') == [2, 1000, 1, 5000, nil, 0]
    receipts = observation['mock_receipts']
    row = {'platform' => 'rust-native-worker', 'case_id' => task['case_id'], 'scenario' => File.basename(directory), 'checks' => checks,
           'native_status_retained' => observation['verification'], 'native_completed' => observation['completed'],
           'native_stop_reason' => observation['stop_reason'], 'original_baseline_revision' => baseline['sha256'],
           'contract_sha256' => observation['contract_sha256'],
           'worker_context_prompt_is_compiled_prelude' => true,
           'request_context_projection_is_derived' => !observation['request_context_projection'].nil?,
           'native_artifact_sha256' => before.to_h { |path, digest| [path.delete_prefix(directory + '/'), digest] }}
    if ordinary_case
      require!(observation['completed'] == true && observation['stop_reason'] == 'completed', 'ordinary_rust_case_not_completed')
      raw_path = File.join(directory, 'raw-model-content.json')
      report_path = File.join(directory, 'raw-report.json')
      raw = File.read(raw_path, encoding: Encoding::UTF_8)
      require!(raw == observation['raw_model_content'] && raw == observation['raw_report'] && raw == File.read(report_path, encoding: Encoding::UTF_8), 'rust_raw_report_changed')
      require!(evidence.values_at('read_observed', 'delivered_to_model', 'fixture_unchanged') == [true, true, true] && observation['format_valid'] == true, 'ordinary_rust_evidence_missing')
      checks['successful_native_read_and_delivery_receipts'] = observation.fetch('events').any? { |event| event['kind'] == 'tool_completed' && event['name'] == 'read_file' && event['is_error'] == false } &&
        receipts.any? { |receipt| receipt['delivered_to_model'] == true && receipt['tool_message_sha256'].is_a?(String) && receipt['tool_message_sha256'].match?(/\A[0-9a-f]{64}\z/) }
      require!(checks['successful_native_read_and_delivery_receipts'], 'ordinary_rust_read_receipt_missing')
      require!(receipts.all? { |receipt| receipt['status'] == 'completed' && receipt.dig('usage', 'total_tokens') == 50 }, 'ordinary_rust_usage_missing')
      prefix = File.join(output, 'rust-' + File.basename(directory))
      stdout, status = Open3.capture2e('ruby', File.join(AgentRsiReproduction::ROOT, 'scripts/lib/agent_rsi_reproduction_eval.rb'),
        '--task', task['case_id'], '--report', raw_path, '--evidence', File.join(directory, 'evidence.json'), '--output-prefix', prefix)
      require!(status.success?, 'rust_host_cli_failed')
      verdict = JSON.parse(File.read(prefix + '.json'))
      require!(verdict['passed'] == true && verdict['report_sha256'] == Digest::SHA256.hexdigest(raw), 'rust_host_verification_failed')
      row.merge!('host_verification' => verdict, 'host_cli_summary' => JSON.parse(stdout), 'scored_by_host' => true)
    else
      stop = observation['stop_reason']
      require!(%w[completed request_limit token_admission token_exhausted timeout cancelled].include?(stop), 'unknown_rust_resource_stop')
      case stop
      when 'request_limit'
        require!(observation['completed'] == false && receipts.length == limits['max_requests'] && receipts.all? { |receipt| receipt['status'] == 'completed' && !receipt['usage'].nil? }, 'request_limit_receipt_boundary')
      when 'token_admission'
        require!(observation['completed'] == false && receipts.empty? && limits['max_tokens'] < 50, 'pre_admission_has_no_physical_request')
      when 'token_exhausted'
        require!(observation['completed'] == false && !receipts.empty? && receipts.all? { |receipt| receipt['status'] == 'completed' && !receipt['usage'].nil? } &&
          receipts.sum { |receipt| receipt['usage']['total_tokens'] } >= limits['max_tokens'], 'consumed_token_limit_not_zero_or_unknown')
      when 'timeout', 'cancelled'
        require!(observation['completed'] == false && receipts.any? { |receipt| receipt['status'] == 'interrupted_usage_unknown' && receipt['usage'].nil? } &&
          observation['worker_lifecycle'].any? { |event| event['phase'] == 'interrupted' && event['outcome'] == 'interrupted' }, 'inflight_usage_unknown_must_remain_null')
      when 'completed'
        require!(observation['completed'] == true && limits['max_requests'] == 4 && limits['read_rounds'] == 3 && receipts.length == 4 && receipts.all? { |receipt| receipt['status'] == 'completed' }, 'four_request_resource_case_boundary')
      end
      if observation['completed'] == false
        require!(observation['raw_report'].nil? && !File.exist?(File.join(directory, 'raw-report.json')), 'unfinished_case_must_not_be_scored')
      end
      row.merge!('scored_by_host' => false, 'host_verification' => nil, 'resource_checks_passed' => true,
                 'usage_unknown_retained' => receipts.any? { |receipt| receipt['usage'].nil? },
                 'mock_requests_admitted' => receipts.length,
                 'claim_limit' => 'Resource acceptance only; never passed into objective quality/reproduction grading.')
    end
    require!(before.all? { |path, digest| Digest::SHA256.file(path).hexdigest == digest }, 'original_rust_artifact_changed')
    row
  end

  def run(argv)
    options = {}
    OptionParser.new do |parser|
      parser.on('--mac-root PATH') { |value| options[:mac] = value }
      parser.on('--rust-root PATH') { |value| options[:rust] = value }
      parser.on('--output-dir PATH') { |value| options[:output] = value }
    end.parse!(argv)
    require!(argv.empty? && options[:mac] && options[:output] && !File.exist?(options[:output]), 'new_output_and_native_root_required')
    options[:output] = File.join(File.realpath(File.dirname(options[:output])), File.basename(options[:output]))
    [options[:mac], options[:rust]].compact.each do |input_root|
      input_root = File.realpath(input_root)
      require!(options[:output] != input_root && !options[:output].start_with?(input_root + '/'), 'output_must_be_outside_native_inputs')
    end
    manifest = JSON.parse(File.read(File.join(AgentRsiReproduction::FIXTURE_ROOT, 'manifest.json')))
    baseline = JSON.parse(File.read(File.join(AgentRsiReproduction::ROOT, 'docs/examples/agent-rsi-golden.json'))).dig('values', 'candidate', 'baseline_package')
    contract = JSON.parse(File.read(File.join(AgentRsiReproduction::ROOT, 'bench/agent-rsi/review-output-v2/contract.json')))
    FileUtils.mkdir_p(options[:output], mode: 0o700)
    rows = manifest['tasks'].map { |task| mac_case(options[:mac], task, options[:output], baseline, contract) }
    resources = []
    if options[:rust]
      directories = Dir[File.join(options[:rust], '*')].select { |path| File.directory?(path) && File.file?(File.join(path, 'observation.json')) }.sort
      require!(directories.length == 8, 'expected_two_rust_success_and_six_resource_directories')
      rust_rows = directories.map do |directory|
        case_id = JSON.parse(File.read(File.join(directory, 'observation.json'))).fetch('case_id')
        task = manifest['tasks'].find { |item| item['case_id'] == case_id }
        require!(!task.nil?, 'unknown_rust_case')
        rust_case(directory, task, options[:output], baseline, contract)
      end
      successes, resources = rust_rows.partition { |row| row['scored_by_host'] }
      require!(successes.length == 2 && successes.map { |row| row['case_id'] }.sort == %w[count-zero-bug count-zero-clean] && resources.length == 6, 'rust_success_resource_counts_changed')
      rows.concat(successes)
    end
    report = {'schema' => 'willdeep.agent-rsi.native-free-host-acceptance.v1', 'created_at' => Time.now.utc.iso8601,
              'native_cases' => rows.length, 'host_passed' => rows.count { |row| row['host_verification']['passed'] },
              'model_calls' => 0, 'cost_microusd' => 0, 'native_observations_modified' => false,
              'paid_trials_modified' => false, 'quality_improvement_claim' => false, 'automatic_promotion' => false,
              'objective_host_function_executions' => rows.sum { |row| row['host_verification']['objective_execution_count'] },
              'resource_cases_unscored' => resources.length, 'resource_cases' => resources, 'cases' => rows}
    File.write(File.join(options[:output], 'native-host-acceptance.json'), JSON.pretty_generate(report) + "\n")
    table = rows.map { |row| "| #{row['platform']} | #{row['case_id']} | #{row['native_status_retained']} | #{row['checks'].length} | #{row['host_verification']['reproduction_status']} | #{row['host_verification']['passed']} |" }
    markdown = ['# 原生 mock → 独立宿主复现验收', '',
                "#{rows.length} 个原生案例通过 #{rows.sum { |row| row['checks'].length }} 项绑定核对，host #{report['host_passed']}/#{rows.length} 通过，实际执行可信 fixture/reference 共 #{report['objective_host_function_executions']} 次。", '',
                '| 平台 | 案例 | 原生状态保留 | 绑定检查 | 宿主复现 | 宿主通过 |', '|---|---|---|---|---|---|', *table, '',
                '原生 observation 保留 unverified，host verdict 独立写入本报告；不覆盖任何原始文件或旧失败记录。核对原 baseline revision/指令、共享契约 bytes/指令、raw 原文 hash、host evidence 一致性、root/worker 血缘、实际 read_file 收据与文件未变。Rust 原始 Worker context 绑定编译 prelude；request_context_projection 明确为首已准入请求派生投影，实际 effective-prompt 与每个 mock receipt 的 system hash 另行核对。', '',
                "资源案例 #{resources.length} 个，仅检查停止/物理请求边界及未知 usage 保留，不做宿主复现评分（包括用于验证四请求足够完成的资源对照）。", '',
                '模型调用 0、费用 0；只证明公开 mock 工程链路，不构成能力提升或晋升结论。', ''].join("\n")
    File.write(File.join(options[:output], 'native-host-acceptance.md'), markdown)
    puts JSON.generate(report.slice('native_cases', 'host_passed', 'objective_host_function_executions', 'model_calls', 'cost_microusd'))
  end
end

AgentRsiReproductionNativeAcceptance.run(ARGV) if $PROGRAM_NAME == __FILE__
