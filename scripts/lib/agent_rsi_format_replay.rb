# frozen_string_literal: true
require 'fileutils'
require 'find'
require 'time'
require_relative 'agent_rsi_grader'

module AgentRsiFormatReplay
  class Denied < StandardError; end
  QUALITY = %w[passed quality_failure].freeze
  KNOWN = (QUALITY + ['budget_exhausted']).freeze
  METRICS = %w[schema_valid classification_correct localization_correct read_observed fixture_unchanged passed].freeze
  IDENTITY = %w[platform task_id split arm repetition attempt resume_of agent_revision].freeze
  ROOT = File.expand_path('../..', __dir__)
  GRADER_PATH = File.join(__dir__, 'agent_rsi_grader.rb')
  POLICY_PATH = File.join(ROOT, 'docs/decisions/2026-10-15-agent-rsi-format-replay-policy.md')
  module_function

  def hash(value) = ContractGolden.hash(value)
  def require!(condition, message) = (raise Denied, message unless condition)

  def inventory(root)
    entries = {}
    Find.find(root) do |path|
      relative = path.delete_prefix(root + '/')
      next if path == root
      stat = File.lstat(path)
      entries[relative] = if stat.symlink?
        {'type' => 'symlink', 'target_sha256' => Digest::SHA256.hexdigest(File.readlink(path))}
      elsif stat.file?
        {'type' => 'file', 'bytes' => stat.size, 'sha256' => Digest::SHA256.file(path).hexdigest}
      elsif stat.directory?
        {'type' => 'directory'}
      else
        raise Denied, 'unexpected non-file in saved batch'
      end
    end
    entries
  end

  class Replay
    def initialize(batch:, audit:, frozen_source:, expected_parser_sha256:)
      @batch, @source = File.realpath(batch), File.realpath(frozen_source)
      @audit = File.realpath(audit)
      @parser_sha = expected_parser_sha256
      @external = {}
    end

    def read(path)
      bytes = File.binread(path)
      @external[path] = Digest::SHA256.hexdigest(bytes)
      JSON.parse(bytes)
    end

    def trial_path(id, file)
      AgentRsiFormatReplay.require!(id.is_a?(String) && id.match?(/\A[0-9a-f-]{36}\z/), 'invalid trial identity')
      path = File.realpath(File.join(@batch, id, file))
      AgentRsiFormatReplay.require!(path.start_with?(@batch + '/'), 'trial input escapes saved batch')
      path
    end

    def prepare
      AgentRsiFormatReplay.require!(Digest::SHA256.file(GRADER_PATH).hexdigest == @parser_sha, 'parser differs from preflight freeze')
      [GRADER_PATH, POLICY_PATH, __FILE__, File.join(__dir__, 'agent_rsi_contract.rb')].each do |path|
        @external[path] = Digest::SHA256.file(path).hexdigest
      end
      @before = AgentRsiFormatReplay.inventory(@batch)
      @ledger, @manifest, @plan = %w[ledger.json manifest.json plan.json].map { |name| read(File.join(@batch, name)) }
      audit = read(@audit)
      check_batch!(audit)
      @manifest.fetch('source_files').each do |relative, expected|
        path = File.realpath(File.join(@source, relative))
        AgentRsiFormatReplay.require!(path.start_with?(@source + '/') && Digest::SHA256.file(path).hexdigest == expected, 'original frozen source changed')
        @external[path] = expected
      end
      @suite = read(@manifest.fetch('suite_path'))
      AgentRsiFormatReplay.require!(AgentRsiFormatReplay.hash(@suite) == @manifest['suite_sha256'], 'original suite changed')
      @derived = @plan.filter_map do |planned|
        row = @ledger['attempts'].find { |attempt| attempt['trial_id'] == planned['trial_id'] }
        replay_trial(row) if QUALITY.include?(row['status'])
      end
      AgentRsiFormatReplay.require!(@external.all? { |p, sha| Digest::SHA256.file(p).hexdigest == sha }, 'bound inputs changed during replay')
      AgentRsiFormatReplay.require!(Digest::SHA256.file(GRADER_PATH).hexdigest == @parser_sha, 'parser changed during replay')
      AgentRsiFormatReplay.require!(@before == AgentRsiFormatReplay.inventory(@batch), 'saved batch changed during replay')
      summary(audit)
    end

    def check_batch!(audit)
      ok = audit['mode'] == 'final' && audit['passed'] == true && audit['issues'] == [] &&
        audit['known_planned_terminals'] == 288 && audit['unknown_requests'] == 0 && audit['inflight_or_unreconciled_requests'] == 0
      AgentRsiFormatReplay.require!(ok, 'a passing final audit is required before reading saved answers')
      AgentRsiFormatReplay.require!(audit['ledger_raw_sha256'] == @external.fetch(File.join(@batch, 'ledger.json')) &&
        audit['manifest_sha256'] == AgentRsiFormatReplay.hash(@manifest) &&
        @ledger['manifest_sha256'] == audit['manifest_sha256'], 'audit is not bound to this unchanged batch')
      AgentRsiFormatReplay.require!(@ledger.key?('paused') && @ledger['paused'].nil?, 'unfinished batch is not replayable')
      AgentRsiFormatReplay.require!(@plan.length == 288 && @plan.map { |x| x['trial_id'] }.uniq.length == 288 &&
        @plan.map { |x| x.values_at('platform', 'task_id', 'arm', 'repetition') }.uniq.length == 288 &&
        AgentRsiFormatReplay.hash(@plan) == @manifest['plan_sha256'], 'frozen plan identity changed')
      @plan.each do |planned|
        rows = @ledger['attempts'].select { |row| row['trial_id'] == planned['trial_id'] }
        AgentRsiFormatReplay.require!(rows.length == 1 && KNOWN.include?(rows.first['status']) &&
          IDENTITY.all? { |key| rows.first[key] == planned[key] }, 'missing or changed planned terminal')
      end
      AgentRsiFormatReplay.require!(@ledger['attempts'].all? { |row| (KNOWN + ['infrastructure_error']).include?(row['status']) }, 'unresolved attempt')
      requests = @ledger.fetch('requests')
      AgentRsiFormatReplay.require!(requests.all? { |r| r['status'] == 'settled' && r['verified_bill_microusd'].is_a?(Integer) && r['verified_bill_microusd'] >= 0 } &&
        requests.length == audit['requests'] && requests.sum { |r| r['verified_bill_microusd'] } == audit['verified_bills_microusd'], 'unknown or changed billing')
    end

    def replay_trial(row)
      id = row.fetch('trial_id')
      observation = read(trial_path(id, 'observation.json'))
      original = read(trial_path(id, 'verifier.json'))
      task = @suite.fetch('tasks').find { |t| t['task_id'] == row['task_id'] }
      AgentRsiFormatReplay.require!(task && task['split'] == row['split'], 'task missing from frozen suite')
      expected = JSON.parse(task.fetch('expected_output'))
      raw = observation.fetch('raw_model_content')
      raw_sha = Digest::SHA256.hexdigest(raw)
      # Frozen Mac v1 has no raw-content digest field. Its original host
      # verifier still binds both the complete observation and the raw text.
      native_hash_valid = if observation.key?('raw_model_content_sha256')
        observation['raw_model_content_sha256'] == raw_sha
      else
        row['platform'] == 'macos-native-worker' && observation['schema'] == 'willdeep.agent-rsi.native-trial-observation.v1' &&
          observation.dig('runtime', 'adapter') == 'macos-native-worker.v1'
      end
      AgentRsiFormatReplay.require!(observation['did_truncate'] == false && observation['trial_id'] == id && original['trial_id'] == id &&
        original['observation_sha256'] == AgentRsiFormatReplay.hash(observation) &&
        original['raw_model_sha256'] == raw_sha && native_hash_valid &&
        original['oracle_sha256'] == AgentRsiFormatReplay.hash(expected) && original['passed'] == (row['status'] == 'passed'), 'original verifier/answer/oracle binding changed')
      %w[read_observed fixture_unchanged].each do |key|
        AgentRsiFormatReplay.require!([true, false].include?(original[key]) && original[key] == observation[key], 'original evidence gate changed')
      end
      derived = AgentRsiLive::Grader.grade(raw, expected, read_observed: original['read_observed'], fixture_unchanged: original['fixture_unchanged'])
      {'trial_id' => id, 'platform' => row['platform'], 'arm' => row['arm'], 'split' => row['split'],
       'original_scoring_epoch' => row.fetch('scoring_protocol', 'v1-historical'),
       'original_verifier_file_sha256' => @external.fetch(trial_path(id, 'verifier.json')),
       'observation_file_sha256' => @external.fetch(trial_path(id, 'observation.json')),
       'original' => original.slice(*METRICS), 'derived_posthoc' => derived,
       'formal_score_replaced' => false}
    end

    def summary(audit)
      groups = @derived.group_by { |r| r.values_at('platform', 'arm', 'original_scoring_epoch') }.map do |(platform, arm, epoch), rows|
        {'platform' => platform, 'arm' => arm, 'original_scoring_epoch' => epoch, 'replayed' => rows.length,
         'original_metrics' => counts(rows, 'original'), 'posthoc_metrics' => counts(rows, 'derived_posthoc')}
      end
      regressions = @derived.count { |r| METRICS.any? { |k| r['original'][k] == true && r['derived_posthoc'][k] != true } }
      network_loaded = $LOADED_FEATURES.any? { |p| p.match?(/net\/http|agent_rsi_live(?:_account|_host)?\.rb/) }
      AgentRsiFormatReplay.require!(!network_loaded, 'offline runner must not load network or paid-runner dependencies')
      {'schema' => 'willdeep.agent-rsi.posthoc-format-replay.v1', 'created_at' => Time.now.utc.iso8601,
       'parser_revision' => AgentRsiLive::Grader::REVISION, 'parser_sha256' => @parser_sha,
       'policy_sha256' => Digest::SHA256.file(POLICY_PATH).hexdigest, 'replay_tool_sha256' => Digest::SHA256.file(__FILE__).hexdigest,
       'original_audit_sha256' => @external.fetch(@audit), 'original_ledger_raw_sha256' => audit['ledger_raw_sha256'],
       'original_manifest_sha256' => audit['manifest_sha256'], 'original_batch_tree_sha256' => AgentRsiFormatReplay.hash(@before),
       'original_batch_entries_verified_unchanged' => @before.length, 'original_inputs_unchanged' => true,
       'planned_terminals' => @plan.length, 'replayed_quality_observations' => @derived.length,
       'budget_terminals_excluded' => @ledger['attempts'].count { |r| r['status'] == 'budget_exhausted' },
       'historical_infrastructure_attempts_excluded' => @ledger['attempts'].count { |r| r['status'] == 'infrastructure_error' },
       'original_model_requests_unchanged' => @ledger['requests'].length,
       'original_verified_cost_microusd_unchanged' => audit['verified_bills_microusd'],
       'original_metrics' => counts(@derived, 'original'), 'posthoc_metrics' => counts(@derived, 'derived_posthoc'),
       'transitions' => METRICS.to_h { |k| [k, {'false_to_true' => @derived.count { |r| r['original'][k] == false && r['derived_posthoc'][k] == true },
         'true_to_false' => @derived.count { |r| r['original'][k] == true && r['derived_posthoc'][k] == false }}] },
       'posthoc_rejection_reasons' => @derived.reject { |r| r['derived_posthoc']['schema_valid'] }.map do |r|
         f = r['derived_posthoc']['format_diagnostic']
         f['error'] || (f['complete_objects'] > 1 ? 'multiple_complete_objects' : 'missing_or_invalid_answer_schema')
       end.tally,
       'groups_by_original_epoch' => groups, 'regressed_observations' => regressions, 'validation_passed' => regressions.zero?,
       'model_requests' => 0, 'network_dependencies_loaded' => false, 'formal_scores_replaced' => false,
       'paid_evaluation_rerun' => false, 'candidate_promotion_allowed' => false,
       'claim_limit' => 'Post-hoc format validation on saved answers, not new independent quality evidence or an improvement/promotion claim.'}
    end

    def counts(rows, field) = METRICS.to_h { |key| [key, rows.count { |r| r[field][key] == true }] }

    def write(output, report)
      output = File.join(File.realpath(File.dirname(output)), File.basename(output))
      AgentRsiFormatReplay.require!(!File.exist?(output) && !File.symlink?(output) &&
        !output.start_with?(@batch + '/') && !output.start_with?(@source + '/'), 'output must be a new separate directory')
      Dir.mkdir(output, 0o700)
      Dir.mkdir(File.join(output, 'derived-verifiers'), 0o700)
      @derived.each { |r| save(File.join(output, 'derived-verifiers', r['trial_id'] + '.json'), r) }
      save(File.join(output, 'original-input-inventory.json'), @before)
      AgentRsiFormatReplay.require!(@before == AgentRsiFormatReplay.inventory(@batch), 'saved input changed while writing results')
      save(File.join(output, 'summary.json'), report)
      File.write(File.join(output, 'summary.md'), markdown(report), mode: 'wx', perm: 0o600)
      report
    end

    def save(path, value) = File.write(path, JSON.pretty_generate(value) + "\n", mode: 'wx', perm: 0o600)

    def markdown(r)
      lines = ['# 已保存回答的免费格式回放', '',
        '**事后格式诊断，原始评分全部保留；不是新的付费评测，也不能据此晋升候选。**', '',
        "解析器：#{r['parser_revision']}；回放 #{r['replayed_quality_observations']} 份可评分回答；排除 #{r['budget_terminals_excluded']} 个预算终态与 #{r['historical_infrastructure_attempts_excluded']} 次历史基础设施失败。", '',
        "新增模型请求：0。原 #{r['original_model_requests_unchanged']} 个请求与 #{r['original_verified_cost_microusd_unchanged']} 微美元实扣保持不变。原评测目录 #{r['original_batch_entries_verified_unchanged']} 个目录项逐项校验未改。", '',
        '| 指标 | 原始计数（不覆盖） | 新解析器免费回放 | false → true | true → false |', '|---|---:|---:|---:|---:|']
      METRICS.each { |k| lines << "| #{k} | #{r['original_metrics'][k]} | #{r['posthoc_metrics'][k]} | #{r['transitions'][k]['false_to_true']} | #{r['transitions'][k]['true_to_false']} |" }
      lines += ['', '## 仍拒绝的格式', '']
      r['posthoc_rejection_reasons'].each { |k, v| lines << "- #{k}：#{v}。" }
      lines += ['', '按原评分 epoch、平台和臂分别汇总于 JSON；不将不同正式协议合成原首轮成绩。分类、精确行、读取和 fixture 门禁保持原样，不评判理由语义或泛化能力。', '']
      lines.join("\n")
    end
  end
end
