#!/usr/bin/env ruby
# frozen_string_literal: true
require 'minitest/autorun'
require 'tmpdir'
require 'securerandom'
require_relative 'lib/agent_rsi_format_replay'
require_relative 'lib/agent_rsi_free_test_report'
AgentRsiFreeTestReport.enable('offline-replay')

class AgentRsiFormatReplayTest < Minitest::Test
  ANSWER = '{"has_defect":true,"line":3,"reason":"zero must remain valid"}'

  def write(path, data)
    File.write(path, data.is_a?(String) ? data : JSON.pretty_generate(data) + "\n")
  end

  def setup
    @root = Dir.mktmpdir('rsi-replay-synthetic-')
    @batch, @source = %w[batch frozen].map { |p| File.join(@root, p).tap { |d| Dir.mkdir(d) } }
    @plan = (0...24).flat_map do |task|
      %w[rust-native-worker macos-native-worker].flat_map do |platform|
        %w[baseline candidate].flat_map do |arm|
          (1..3).map do |rep|
            {'trial_id' => SecureRandom.uuid, 'task_id' => "fixture-#{task}", 'platform' => platform,
             'split' => %w[validation regression holdout][task % 3], 'arm' => arm, 'repetition' => rep,
             'attempt' => 1, 'resume_of' => nil, 'agent_revision' => arm}
          end
        end
      end
    end
    expected = {'has_defect' => true, 'line' => 3}
    suite = {'tasks' => (0...24).map { |i| {'task_id' => "fixture-#{i}", 'split' => %w[validation regression holdout][i % 3], 'expected_output' => JSON.generate(expected)} }}
    suite_path = File.join(@source, 'suite.json')
    write(suite_path, suite)
    write(File.join(@source, 'frozen.rb'), '# synthetic frozen source')
    manifest = {'plan_sha256' => AgentRsiFormatReplay.hash(@plan), 'suite_sha256' => AgentRsiFormatReplay.hash(suite),
      'suite_path' => suite_path, 'source_files' => {'frozen.rb' => Digest::SHA256.file(File.join(@source, 'frozen.rb')).hexdigest}}
    @ledger = {'manifest_sha256' => AgentRsiFormatReplay.hash(manifest), 'paused' => nil,
      'attempts' => @plan.map.with_index { |r, i| r.merge('status' => i.zero? ? 'quality_failure' : 'budget_exhausted') }, 'requests' => []}
    write(File.join(@batch, 'manifest.json'), manifest)
    write(File.join(@batch, 'plan.json'), @plan)
    @id = @plan.first['trial_id']
    Dir.mkdir(File.join(@batch, @id))
    raw = "#{ANSWER}\n```ts\nfunction f() { return {}; }\n```"
    @observation = {'trial_id' => @id, 'did_truncate' => false, 'raw_model_content' => raw,
      'raw_model_content_sha256' => Digest::SHA256.hexdigest(raw), 'read_observed' => true, 'fixture_unchanged' => true}
    @verifier = {'trial_id' => @id, 'passed' => false, 'schema_valid' => false,
      'classification_correct' => false, 'localization_correct' => false, 'read_observed' => true, 'fixture_unchanged' => true,
      'observation_sha256' => AgentRsiFormatReplay.hash(@observation),
      'raw_model_sha256' => Digest::SHA256.hexdigest(raw), 'oracle_sha256' => AgentRsiFormatReplay.hash(expected)}
    write(File.join(@batch, @id, 'observation.json'), @observation)
    write(File.join(@batch, @id, 'verifier.json'), @verifier)
    @audit_path = File.join(@root, 'final-audit.json')
    @audit = {'mode' => 'final', 'passed' => true, 'issues' => [], 'known_planned_terminals' => 288,
      'unknown_requests' => 0, 'inflight_or_unreconciled_requests' => 0,
      'manifest_sha256' => AgentRsiFormatReplay.hash(manifest), 'requests' => 0, 'verified_bills_microusd' => 0}
    save_ledger
  end

  def save_ledger
    write(File.join(@batch, 'ledger.json'), @ledger)
    @audit['ledger_raw_sha256'] = Digest::SHA256.file(File.join(@batch, 'ledger.json')).hexdigest
    write(@audit_path, @audit)
  end

  def teardown = FileUtils.remove_entry(@root)
  def replay(sha = Digest::SHA256.file(AgentRsiFormatReplay::GRADER_PATH).hexdigest)
    AgentRsiFormatReplay::Replay.new(batch: @batch, audit: @audit_path, frozen_source: @source, expected_parser_sha256: sha)
  end

  def test_complete_offline_replay_preserves_originals_and_excludes_budgets
    before = AgentRsiFormatReplay.inventory(@batch)
    runner = replay
    result = runner.prepare
    assert_equal 1, result['replayed_quality_observations']
    assert_equal 287, result['budget_terminals_excluded']
    assert_equal 0, result['original_metrics']['passed']
    assert_equal 1, result['posthoc_metrics']['passed']
    assert_equal 0, result['regressed_observations']
    assert_equal 0, result['model_requests']
    refute result['formal_scores_replaced']
    out = File.join(@root, 'derived')
    runner.write(out, result)
    assert_equal before, AgentRsiFormatReplay.inventory(@batch)
    assert File.file?(File.join(out, 'derived-verifiers', @id + '.json'))
    assert_equal @verifier, JSON.parse(File.read(File.join(@batch, @id, 'verifier.json')))
  end

  def test_bad_parser_hash_rejected
    assert_raises(AgentRsiFormatReplay::Denied) { replay('0' * 64).prepare }
  end

  def test_failed_audit_rejected_before_answer_read
    @audit['passed'] = false
    write(@audit_path, @audit)
    File.unlink(File.join(@batch, @id, 'observation.json'))
    assert_match(/passing final audit/, assert_raises(AgentRsiFormatReplay::Denied) { replay.prepare }.message)
  end

  def test_duplicate_plan_identity_rejected
    @plan[1] = @plan[0]
    write(File.join(@batch, 'plan.json'), @plan)
    assert_raises(AgentRsiFormatReplay::Denied) { replay.prepare }
  end

  def test_pause_rejected
    @ledger['paused'] = 'unknown cost'
    save_ledger
    assert_raises(AgentRsiFormatReplay::Denied) { replay.prepare }
  end

  def test_unknown_bill_rejected
    @ledger['requests'] << {'status' => 'unknown'}
    save_ledger
    assert_raises(AgentRsiFormatReplay::Denied) { replay.prepare }
  end

  def test_missing_terminal_rejected
    @ledger['attempts'].pop
    save_ledger
    assert_raises(AgentRsiFormatReplay::Denied) { replay.prepare }
  end

  def test_frozen_source_change_rejected
    write(File.join(@source, 'frozen.rb'), '# modified')
    assert_raises(AgentRsiFormatReplay::Denied) { replay.prepare }
  end

  def test_raw_answer_change_rejected
    @observation['raw_model_content'] += ' changed'
    write(File.join(@batch, @id, 'observation.json'), @observation)
    assert_raises(AgentRsiFormatReplay::Denied) { replay.prepare }
  end

  def bind_observation
    @verifier['observation_sha256'] = AgentRsiFormatReplay.hash(@observation)
    write(File.join(@batch, @id, 'observation.json'), @observation)
    write(File.join(@batch, @id, 'verifier.json'), @verifier)
  end

  def mac_schema
    @plan.first['platform'] = 'macos-native-worker'
    @plan.first['task_id'] = 'fixture-24'
    @ledger['attempts'].first.merge!(@plan.first)
    # Keep this synthetic change unique without touching the other 287 slots.
    suite_path = File.join(@source, 'suite.json')
    suite = JSON.parse(File.read(suite_path))
    suite['tasks'] << suite['tasks'].first.merge('task_id' => 'fixture-24')
    write(suite_path, suite)
    manifest_path = File.join(@batch, 'manifest.json')
    manifest = JSON.parse(File.read(manifest_path))
    manifest['plan_sha256'] = AgentRsiFormatReplay.hash(@plan)
    manifest['suite_sha256'] = AgentRsiFormatReplay.hash(suite)
    write(File.join(@batch, 'plan.json'), @plan)
    write(manifest_path, manifest)
    @audit['manifest_sha256'] = @ledger['manifest_sha256'] = AgentRsiFormatReplay.hash(manifest)
    save_ledger
    @observation.delete('raw_model_content_sha256')
    @observation['schema'] = 'willdeep.agent-rsi.native-trial-observation.v1'
    @observation['runtime'] = {'adapter' => 'macos-native-worker.v1'}
    bind_observation
  end

  def test_frozen_mac_schema_without_optional_digest_uses_both_original_bindings
    mac_schema
    assert_equal 1, replay.prepare['posthoc_metrics']['passed']
  end

  def test_mac_explicit_null_digest_not_treated_as_absent
    mac_schema
    @observation['raw_model_content_sha256'] = nil
    bind_observation
    assert_raises(AgentRsiFormatReplay::Denied) { replay.prepare }
  end

  def test_mac_without_native_digest_still_rejects_wrong_verifier_raw_digest
    mac_schema
    @verifier['raw_model_sha256'] = '0' * 64
    write(File.join(@batch, @id, 'verifier.json'), @verifier)
    assert_raises(AgentRsiFormatReplay::Denied) { replay.prepare }
  end

  def test_mac_unknown_adapter_not_accepted_for_missing_digest
    mac_schema
    @observation['runtime']['adapter'] = 'unknown'
    bind_observation
    assert_raises(AgentRsiFormatReplay::Denied) { replay.prepare }
  end

  def test_missing_rust_digest_rejected_even_if_observation_is_bound
    @observation.delete('raw_model_content_sha256')
    bind_observation
    assert_raises(AgentRsiFormatReplay::Denied) { replay.prepare }
  end

  def test_oracle_change_rejected
    @verifier['oracle_sha256'] = '0' * 64
    write(File.join(@batch, @id, 'verifier.json'), @verifier)
    assert_raises(AgentRsiFormatReplay::Denied) { replay.prepare }
  end

  def test_observation_and_gate_change_rejected
    @verifier['read_observed'] = false
    write(File.join(@batch, @id, 'verifier.json'), @verifier)
    assert_raises(AgentRsiFormatReplay::Denied) { replay.prepare }
  end

  def test_output_inside_original_batch_rejected
    runner = replay
    result = runner.prepare
    assert_raises(AgentRsiFormatReplay::Denied) { runner.write(File.join(@batch, 'replay'), result) }
    refute File.exist?(File.join(@batch, 'replay'))
  end

  def test_existing_output_never_overwritten
    runner = replay
    result = runner.prepare
    out = File.join(@root, 'existing')
    Dir.mkdir(out)
    write(File.join(out, 'keep'), 'keep')
    assert_raises(AgentRsiFormatReplay::Denied) { runner.write(out, result) }
    assert_equal 'keep', File.read(File.join(out, 'keep'))
  end

  def test_symlink_escape_rejected
    original = File.join(@batch, @id, 'observation.json')
    external = File.join(@root, 'outside.json')
    File.rename(original, external)
    File.symlink(external, original)
    assert_raises(AgentRsiFormatReplay::Denied) { replay.prepare }
  end
end
