# frozen_string_literal: true
require 'minitest/autorun'
require 'tmpdir'
require 'fileutils'
require_relative 'lib/agent_rsi_reproduction'
require_relative 'test_agent_rsi_reproduction_native'

class AgentRsiReproductionTest < Minitest::Test
  Grader = AgentRsiReproduction

  def setup
    @manifest = JSON.parse(File.read(File.join(Grader::FIXTURE_ROOT, 'manifest.json')))
    @task = @manifest['tasks'].find { |task| task['case_id'] == 'count-zero-bug' }
    @report = {'has_defect' => true, 'line' => 3, 'reason' => 'Zero must remain valid.', 'reproduction' => @task['public_witness']}
    @evidence = {'fixture_sha256' => @task['fixture_sha256'], 'read_observed' => true, 'delivered_to_model' => true, 'fixture_unchanged' => true}
  end

  def grade(raw = JSON.generate(@report), evidence: @evidence, task: 'count-zero-bug', **options)
    Grader.grade(raw, task_id: task, evidence: evidence, **options)
  end

  def test_all_six_public_fixtures_execute_fixture_and_reference
    @manifest['tasks'].each do |task|
      report = {'has_defect' => task['has_defect'], 'line' => task['defect_line'], 'reason' => 'Public fixture check.', 'reproduction' => task['public_witness']}
      evidence = @evidence.merge('fixture_sha256' => task['fixture_sha256'])
      result = grade(JSON.generate(report), evidence: evidence, task: task['case_id'])
      assert result['passed'], "#{task['case_id']}: #{result}"
      assert_operator result['objective_execution_count'], :>=, 2
      assert_equal 0, result['model_calls']
      assert_equal 0, result['cost_microusd']
      if task['has_defect']
        assert_equal true, result['reproduction_verified']
        refute_equal result['host_executions'][0]['fixture_actual'], result['host_executions'][0]['reference_expected']
      else
        assert_nil result['reproduction_verified']
        assert_equal 'not_applicable_clean', result['reproduction_status']
      end
    end
  end

  def test_forged_expected_is_rejected_after_actual_execution
    @report['reproduction']['expected'] = 1
    result = grade
    assert result['format_valid']
    assert result['classification_correct']
    assert_equal 2, result['objective_execution_count']
    assert_equal false, result['reproduction_verified']
    refute result['passed']
  end

  def test_forged_actual_is_rejected
    @report['reproduction']['actual'] = 9
    refute grade['reproduction_verified']
  end

  def test_non_triggering_input_cannot_claim_a_defect
    @report['reproduction'] = {'input' => {'count' => 2, 'fallback' => 7}, 'expected' => 2, 'actual' => 2}
    result = grade
    assert_equal 2, result['objective_execution_count']
    refute result['reproduction_verified']
  end

  def test_clean_fixture_cannot_produce_false_positive_witness
    result = grade(evidence: @evidence.merge('fixture_sha256' => @manifest['tasks'][1]['fixture_sha256']), task: 'count-zero-clean')
    refute result['classification_correct']
    refute result['reproduction_verified']
    refute result['passed']
  end

  def test_no_reproduction_is_not_verified
    @report['reproduction'] = nil
    refute grade['format_valid']
    assert_equal 0, grade['objective_execution_count']
  end

  def test_missing_read_evidence_cannot_pass
    result = grade(evidence: {})
    assert result['reproduction_verified']
    refute result['evidence_verified']
    refute result['passed']
  end

  def test_wrong_fixture_evidence_cannot_pass
    result = grade(evidence: @evidence.merge('fixture_sha256' => '0' * 64))
    refute result['read_observed']
    refute result['passed']
  end

  def test_tool_read_not_delivered_to_model_cannot_pass
    refute grade(evidence: @evidence.merge('delivered_to_model' => false))['passed']
  end

  def test_explicitly_unread_fixture_cannot_pass_even_with_correct_witness
    result = grade(evidence: @evidence.merge('read_observed' => false))
    assert result['reproduction_verified']
    refute result['read_observed']
    refute result['evidence_verified']
    refute result['passed']
  end

  def test_host_grader_does_not_load_network_or_paid_runtime
    refute $LOADED_FEATURES.any? { |path| path.match?(/net\/http|agent_rsi_live(?:_account|_host)?\.rb/) }
  end

  def invoke_cli(raw, evidence: JSON.generate(@evidence))
    Dir.mktmpdir do |directory|
      report_path = File.join(directory, 'raw.json')
      evidence_path = File.join(directory, 'evidence.json')
      prefix = File.join(directory, 'host')
      File.binwrite(report_path, raw)
      File.binwrite(evidence_path, evidence)
      output, status = Open3.capture2e('ruby', File.join(__dir__, 'lib/agent_rsi_reproduction_eval.rb'),
        '--task', 'count-zero-bug', '--report', report_path, '--evidence', evidence_path, '--output-prefix', prefix)
      result = File.exist?(prefix + '.json') ? JSON.parse(File.read(prefix + '.json')) : nil
      [status.success?, result, output]
    end
  end

  def test_actual_cli_bounded_read_preserves_unicode_utf8
    @report['reason'] = '零必须保留，不能使用默认值。'
    success, result, output = invoke_cli(JSON.generate(@report))
    assert success, output
    assert result['format_valid']
    assert result['passed']
    assert_equal 2, result['objective_execution_count']
  end

  def test_actual_cli_invalid_utf8_report_still_fails_closed
    success, result, output = invoke_cli("\xFF".b)
    assert success, output
    refute result['format_valid']
    refute result['passed']
    assert_equal 'report_size_or_encoding', result['error_code']
    assert_equal 0, result['objective_execution_count']
  end

  def test_actual_cli_invalid_utf8_host_evidence_is_rejected
    success, result, output = invoke_cli(JSON.generate(@report), evidence: "\xFF".b)
    refute success
    assert_nil result
    assert_includes output, 'not valid UTF-8'
  end

  def prompt_binding_fixture
    baseline = {'definition' => {'instructions' => '原始定义'}}
    contract = {'instructions' => '固定输出契约'}
    instruction_sha = Digest::SHA256.hexdigest(contract['instructions'])
    prelude = "#{baseline['definition']['instructions']}\n#{contract['instructions']}\nOutput instructions SHA-256: #{instruction_sha}"
    prompts = [prelude + "\nRemaining tokens: 1000", prelude + "\nRemaining tokens: 950"]
    context = {'definition_revision' => 'fixed', 'effective_prompt_sha256' => Digest::SHA256.hexdigest(prelude)}
    observation = {'worker_context' => context, 'instructions_sha256' => instruction_sha,
      'effective_prompt_sha256' => Digest::SHA256.hexdigest(prompts.first),
      'mock_receipts' => prompts.map { |prompt| {'system_prompt' => prompt, 'system_prompt_sha256' => Digest::SHA256.hexdigest(prompt)} },
      'request_context_projection' => {'derived' => true, 'source' => 'first_admitted_mock_request',
        'context' => context.merge('effective_prompt_sha256' => Digest::SHA256.hexdigest(prompts.first))}}
    [observation, prompts.first, baseline, contract]
  end

  def test_dynamic_per_request_prompts_bound_individually_not_forced_equal
    inputs = prompt_binding_fixture
    checks = AgentRsiReproductionNativeAcceptance.rust_prompt_checks(*inputs)
    assert checks.values.all?, checks.inspect
    refute_equal inputs[0]['mock_receipts'][0]['system_prompt_sha256'], inputs[0]['mock_receipts'][1]['system_prompt_sha256']
  end

  def test_later_prompt_missing_raw_bytes_is_not_treated_as_verified
    inputs = prompt_binding_fixture
    inputs[0]['mock_receipts'][1].delete('system_prompt')
    refute AgentRsiReproductionNativeAcceptance.rust_prompt_checks(*inputs)['every_admitted_system_prompt_independently_bound']
  end

  def test_later_prompt_forged_hash_is_rejected
    inputs = prompt_binding_fixture
    inputs[0]['mock_receipts'][1]['system_prompt_sha256'] = '0' * 64
    refute AgentRsiReproductionNativeAcceptance.rust_prompt_checks(*inputs)['every_admitted_system_prompt_independently_bound']
  end

  def test_recomputed_hash_cannot_hide_changed_locked_prelude
    inputs = prompt_binding_fixture
    receipt = inputs[0]['mock_receipts'][1]
    receipt['system_prompt'] = receipt['system_prompt'].sub('原始定义', 'changed')
    receipt['system_prompt_sha256'] = Digest::SHA256.hexdigest(receipt['system_prompt'])
    refute AgentRsiReproductionNativeAcceptance.rust_prompt_checks(*inputs)['every_prompt_preserves_locked_prelude_and_instructions']
  end

  def test_request_projection_must_remain_explicitly_derived
    inputs = prompt_binding_fixture
    inputs[0]['request_context_projection']['derived'] = false
    refute AgentRsiReproductionNativeAcceptance.rust_prompt_checks(*inputs)['derived_projection_explicit_and_exact']
  end

  def test_original_worker_context_cannot_be_replaced_with_full_wire_prompt
    inputs = prompt_binding_fixture
    inputs[0]['worker_context']['effective_prompt_sha256'] = inputs[0]['effective_prompt_sha256']
    refute AgentRsiReproductionNativeAcceptance.rust_prompt_checks(*inputs)['compiled_prelude_bound_without_relabeling']
  end

  def test_modified_fixture_evidence_cannot_pass
    refute grade(evidence: @evidence.merge('fixture_unchanged' => false))['passed']
  end

  def test_wrong_line_is_separate_from_verified_reproduction
    @report['line'] = 2
    result = grade
    assert result['reproduction_verified']
    assert result['classification_correct']
    refute result['line_correct']
    refute result['passed']
  end

  def test_model_code_field_is_rejected
    @report['reproduction']['code'] = 'process.exit()'
    refute grade['format_valid']
  end

  def test_model_path_input_is_rejected_without_execution
    @report['reproduction']['input']['path'] = '/etc/passwd'
    result = grade
    assert_equal 'input_shape_or_size', result['error_code']
    assert_equal 0, result['objective_execution_count']
  end

  def test_script_looking_string_is_never_executed
    @report['reproduction']['input']['count'] = 'process.exit()'
    result = grade
    assert_equal 'input_shape_or_size', result['error_code']
    assert_equal 0, result['objective_execution_count']
  end

  def test_reason_is_data_not_executable_code_or_semantic_judgment
    @report['reason'] = 'process.exit(); require("http").get("https://example.invalid")'
    assert grade['passed']
  end

  INVALID_REPORTS = {
    'markdown fence' => "```json\n{\"has_defect\":false,\"line\":null,\"reason\":\"clean\",\"reproduction\":null}\n```",
    'two answers' => '{"has_defect":false,"line":null,"reason":"clean","reproduction":null}' * 2,
    'normal report prose' => 'CONCLUSION {"has_defect":false,"line":null,"reason":"clean","reproduction":null}',
    'duplicate key' => '{"has_defect":false,"has_defect":false,"line":null,"reason":"clean","reproduction":null}',
    'nested duplicate key' => '{"has_defect":true,"line":3,"reason":"why","reproduction":{"input":{"count":0,"count":2,"fallback":7},"expected":0,"actual":7}}',
    'negative zero' => '{"has_defect":true,"line":3,"reason":"why","reproduction":{"input":{"count":-0,"fallback":7},"expected":0,"actual":7}}',
    'float integer' => '{"has_defect":true,"line":3.0,"reason":"why","reproduction":{"input":{"count":0,"fallback":7},"expected":0,"actual":7}}',
    'exponent integer' => '{"has_defect":true,"line":3e0,"reason":"why","reproduction":{"input":{"count":0,"fallback":7},"expected":0,"actual":7}}',
    'unsafe integer' => '{"has_defect":true,"line":9007199254740992,"reason":"why","reproduction":{"input":{"count":0,"fallback":7},"expected":0,"actual":7}}',
    'prototype key' => '{"has_defect":true,"line":3,"reason":"why","reproduction":{"input":{"__proto__":{},"count":0,"fallback":7},"expected":0,"actual":7}}',
    'constructor key' => '{"has_defect":true,"line":3,"reason":"why","reproduction":{"input":{"constructor":{},"count":0,"fallback":7},"expected":0,"actual":7}}',
    'empty reason' => '{"has_defect":false,"line":null,"reason":" ","reproduction":null}',
    'old three field schema' => '{"has_defect":false,"line":null,"reason":"clean"}'
  }.freeze

  INVALID_REPORTS.each do |name, raw|
    define_method('test_reject_' + name.gsub(/\W+/, '_')) do
      result = grade(raw)
      refute result['format_valid']
      assert_equal 0, result['objective_execution_count']
    end
  end

  def test_report_size_limit
    refute grade(' ' * (Grader::MAX_BYTES + 1))['format_valid']
  end

  def test_invalid_utf8
    refute grade("\xFF".dup.force_encoding(Encoding::UTF_8))['format_valid']
  end

  def test_collection_item_limit
    @report['reproduction']['expected'] = Array.new(129, 0)
    assert_equal 'collection_item_limit', grade['error_code']
  end

  def test_oversized_object_key_is_also_a_string_limit
    @report['reproduction']['expected'] = {'k' * 4097 => 0}
    assert_equal 'string_size_limit', grade['error_code']
  end

  def test_aggregate_counts_each_object_member_once_and_each_array_item_once
    value = {'one' => [1, 2], 'two' => {'three' => 3}}
    counter = [0]
    Grader.bounded_data!(value, 1, counter)
    assert_equal 5, counter[0]
  end

  def test_shared_golden_output_contract
    golden = JSON.parse(File.read(File.join(Grader::ROOT, 'bench/agent-rsi/review-output-v2/golden.json')))
    golden['valid'].each { |value| assert Grader.parse_report(JSON.generate(value)) }
    golden['invalid'].each { |value| assert_raises(Grader::Denied) { Grader.parse_report(JSON.generate(value)) } }
    assert_equal 12, golden.fetch('raw_cases').length
    golden['raw_cases'].each do |item|
      accepted = begin
        Grader.parse_report(item.fetch('raw'))
        true
      rescue Grader::Denied
        false
      end
      assert_equal item.fetch('valid'), accepted, item.fetch('id')
    end
  end

  def test_depth_limit
    @report['reproduction']['expected'] = 10.times.reduce(0) { |value, _| [value] }
    refute grade['format_valid']
  end

  def test_untrusted_manifest_is_rejected
    Dir.mktmpdir do |root|
      File.write(File.join(root, 'manifest.json'), '{"tasks":[]}')
      result = grade(fixture_root: root)
      assert_equal 'untrusted_manifest', result['error_code']
      assert_equal 0, result['objective_execution_count']
    end
  end

  def test_changed_fixture_source_is_rejected_even_if_task_id_matches
    Dir.mktmpdir do |root|
      FileUtils.cp_r(File.join(Grader::FIXTURE_ROOT, '.'), root)
      path = File.join(root, @task['fixture_file'])
      File.write(path, File.read(path) + '\nthrow new Error("tamper");')
      assert_equal 'trusted_source_digest_mismatch', grade(fixture_root: root)['error_code']
    end
  end

  def test_unknown_case_cannot_supply_own_source_path
    assert_equal 'unknown_public_case', grade(task: '../../arbitrary.js')['error_code']
  end

  def test_host_negative_fixture_is_not_a_worker_case
    assert_equal 'unknown_public_case', grade(task: 'timeout')['error_code']
  end

  def test_changed_reference_cannot_be_trusted
    Dir.mktmpdir do |root|
      FileUtils.cp_r(File.join(Grader::FIXTURE_ROOT, '.'), root)
      File.write(File.join(root, @task['reference_file']), 'function reviewed(input) { return 123; }')
      assert_equal 'trusted_source_digest_mismatch', grade(fixture_root: root)['error_code']
    end
  end

  def test_source_symlink_cannot_escape_fixture_root
    Dir.mktmpdir do |root|
      FileUtils.cp_r(File.join(Grader::FIXTURE_ROOT, '.'), root)
      path = File.join(root, @task['fixture_file'])
      File.delete(path)
      File.symlink('/etc/hosts', path)
      assert_equal 'untrusted_source_path', grade(fixture_root: root)['error_code']
    end
  end

  def host_negative(name)
    item = @manifest['host_negative_fixtures'].find { |entry| entry['name'] == name }
    source = File.read(File.join(Grader::FIXTURE_ROOT, item['source_file']))
    task = {'fixture_sha256' => item['source_sha256'], 'reference_sha256' => item['source_sha256']}
    [task, source]
  end

  def test_trusted_timeout_fixture_is_actually_terminated
    task, source = host_negative('timeout')
    error = assert_raises(Grader::Denied) { Grader.send(:execute, task, source, source, {}) }
    assert_equal 'trusted_execution_timeout', error.message
  end

  def test_trusted_output_limit_fixture_cannot_overflow_report
    task, source = host_negative('output-limit')
    assert_raises(Grader::Denied) { Grader.send(:execute, task, source, source, {}) }
  end

  def test_vm_has_no_process_or_require_network_entry
    task, source = host_negative('sandbox')
    result = Grader.send(:execute, task, source, source, {})
    assert_equal true, result['fixture_actual']
    assert_equal true, result['reference_expected']
  end

  def test_unlisted_source_rejected_even_with_matching_self_hash
    source = 'function reviewed(input) { return input; }'
    sha = Digest::SHA256.hexdigest(source)
    assert_raises(Grader::Denied) { Grader.send(:execute, {'fixture_sha256' => sha, 'reference_sha256' => sha}, source, source, {}) }
  end
end
