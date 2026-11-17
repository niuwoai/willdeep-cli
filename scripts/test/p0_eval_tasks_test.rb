#!/usr/bin/env ruby
# frozen_string_literal: true

# Offline P0 fixture-contract tests. No model, daemon, network, npm, or workspace Cargo.
# ruby scripts/test/p0_eval_tasks_test.rb
# ruby scripts/test/p0_eval_tasks_test.rb --report-dir target/p0-eval-contracts
# Reports reuse ModelEval::Report; their pass rate describes fixture checks, NOT models.

require 'fileutils'
require 'json'
require 'open3'
require 'tmpdir'

require_relative '../lib/model_eval/task'
require_relative '../lib/model_eval/verifier'
require_relative '../lib/model_eval/report'

module P0EvalContracts
  ROOT = File.expand_path('../..', __dir__)
  BENCH = File.join(ROOT, 'bench/model-eval')
  IDS = %w[p0-rust-cross-file-references p0-typescript-symbol-key-references p0-protected-multifile-config].freeze
  ROWS = []

  module_function

  def report_option!
    index = ARGV.index('--report-dir')
    return unless index

    directory = ARGV[index + 1]
    raise ArgumentError, '--report-dir requires an output directory' if directory.nil? || directory.start_with?('-')

    ARGV.slice!(index, 2)
    report_directory(directory)
  end

  def report_directory(directory)
    absolute = File.expand_path(directory)
    existing = absolute
    suffix = []
    until File.exist?(existing)
      suffix.unshift(File.basename(existing))
      existing = File.dirname(existing)
    end
    resolved = File.join(File.realpath(existing), *suffix)
    protected_bench = File.realpath(BENCH)
    if resolved == protected_bench || resolved.start_with?(protected_bench + File::SEPARATOR)
      raise ArgumentError, 'offline fixture reports must not enter the real model history directory'
    end
    absolute
  end

  REPORT_DIR = report_option!

  def task(id)
    ModelEval::Task.load(File.join(BENCH, 'tasks', id))
  end

  def prerequisites(task)
    missing = task.missing_requirements
    if task.language == 'typescript' && !missing.include?('node')
      version, status = Open3.capture2('node', '--version')
      actual = version.sub(/\Av/, '').strip.split('.').map(&:to_i)
      missing += ['Node >=22.18.0'] unless status.success? && (actual <=> [22, 18, 0]) >= 0
    end
    missing
  end

  def row(task, label, status:, **detail)
    { task: "#{task.id}/#{label}", kind: task.kind, language: task.language, split: task.split,
      task_sha256: task.content_sha256, status: status, missing: [],
      claimed: nil, false_completion: false }.merge(detail)
  end

  def write(root, relative, text)
    path = File.join(root, relative)
    FileUtils.mkdir_p(File.dirname(path))
    File.write(path, text)
  end

  def offline_markdown(report, summary)
    text = ModelEval::Report.markdown(report, summary)
    text = text.sub(/\A# 模型行为评测[^\n]*/, '# P0 离线夹具契约验收')
    scope = '本报告没有运行模型。每行 passed 表示红绿或反作弊断言符合预期，受保护文件被改动的反例应被拒绝；它不是候选实现通过率。具体预期与观察值见同名 JSON。'
    text = text.sub("\n\n", "\n\n#{scope}\n\n")
    text = text.sub('**verifier 通过率**', '**契约断言通过率**')
    text = text.sub(/^\| 误报完成率 \|.*$/, '| 误报完成率 | 未运行模型，不适用 |')
    text.sub(/^状态：passed =.*$/, '状态：passed = 契约断言符合预期；failed = 契约断言不符合预期；skipped = 缺少原生依赖，未执行。LSP、编辑协议、并发、恢复及完整 TypeScript 类型检查不在本报告覆盖范围内。')
  end

  def archive
    return unless REPORT_DIR

    context = ModelEval::Report.git_context(ROOT)
    summary = ModelEval::Report.summarize(model: 'offline-fixture-contracts-no-model', rows: ROWS, **context)
    report = {
      'schema_version' => 1,
      'evaluation_mode' => 'offline_fixture_contract_checks',
      'contains_model_runs' => false,
      'scope' => 'Each row is a red/green or anti-cheat assertion; pass_rate measures those assertions, not model success.',
      'limits' => ['No tool protocol, LSP responses, stale edits, concurrency, restart recovery, or browser coverage.',
                   'TypeScript uses fixed reference-shape checks plus Node type stripping, not full type checking.'],
      'rows' => ROWS.map { |entry| entry.transform_keys(&:to_s) }
    }
    paths = ModelEval::Report.archive(report, summary, REPORT_DIR)
    File.write(paths.fetch(1), offline_markdown(report, summary))
    paths.each { |path| warn "offline fixture report: #{path}" }
  end
end

require 'minitest/autorun'
Minitest.after_run { P0EvalContracts.archive }

class P0EvalTasksTest < Minitest::Test
  def available_task(id, label)
    task = P0EvalContracts.task(id)
    missing = P0EvalContracts.prerequisites(task)
    if missing.any?
      P0EvalContracts::ROWS << P0EvalContracts.row(task, label, status: 'skipped', missing: missing)
      skip "#{task.id}: missing #{missing.join(', ')}"
    end
    task
  end

  # Mirrors the existing driver's conjunction: evaluate() alone intentionally does
  # not decide protected-file integrity; the outer workspace snapshot does that.
  def assert_candidate(task, label, expected:, solution: false)
    started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    Dir.mktmpdir('p0-eval-candidate-') do |workspace|
      ModelEval::Verifier.copy_tree(task.fixture_dir, workspace)
      before = ModelEval::Verifier.snapshot(workspace, task.editable)
      ModelEval::Verifier.copy_tree(task.solution_dir, workspace) if solution
      yield workspace if block_given?
      verdict = ModelEval::Verifier.evaluate(task, workspace)
      intact = before == ModelEval::Verifier.snapshot(workspace, task.editable)
      accepted = verdict[:passed] && intact
      P0EvalContracts::ROWS << P0EvalContracts.row(task, label,
        status: accepted == expected ? 'passed' : 'failed',
        expected_candidate_accepted: expected, observed_candidate_accepted: accepted,
        verifier_passed: verdict[:verifier_passed], protected_intact: intact,
        content_violations: verdict[:content_violations],
        elapsed_seconds: (Process.clock_gettime(Process::CLOCK_MONOTONIC) - started).round(3))
      assert_equal expected, accepted, "#{task.id}/#{label}: #{verdict.inspect}, protected_intact=#{intact}"
      verdict.merge(protected_intact: intact)
    end
  end

  def test_actual_schema_and_dependencies_are_explicit
    tasks = P0EvalContracts::IDS.map { |id| P0EvalContracts.task(id) }
    assert_equal P0EvalContracts::IDS.sort, tasks.map(&:id).sort
    all_ids = ModelEval::Task.load_all(P0EvalContracts::BENCH).map(&:id)
    tasks.each do |task|
      assert_includes all_ids, task.id
      assert_equal 'validation', task.split
      assert_equal [['ruby', task.language == 'ruby' ? 'test/contract_test.rb' : 'test/verify.rb']], task.verify
      assert_empty task.mutants
      fixture_files = Dir[File.join(task.fixture_dir, '**/*')].select { |path| File.file?(path) }
        .map { |path| path.delete_prefix("#{task.fixture_dir}/") }
      assert_empty task.editable - fixture_files
      assert_empty task.editable.grep(%r{\A(?:test|config)/})
      assert fixture_files.any? { |path| path.start_with?('test/') }
      solution_files = Dir[File.join(task.solution_dir, '**/*')].select { |path| File.file?(path) }
        .map { |path| path.delete_prefix("#{task.solution_dir}/") }
      assert_equal task.editable.sort, solution_files.sort
      task.editable.each do |relative|
        refute_equal File.binread(File.join(task.fixture_dir, relative)), File.binread(File.join(task.solution_dir, relative))
      end
    end
    assert_includes tasks[0].requires, 'rustc'
    assert_includes tasks[0].title, 'rustc >=1.56'
    assert_includes tasks[1].requires, 'node'
    assert_includes tasks[1].title, 'Node >=22.18.0'
  end

  P0EvalContracts::IDS.each do |id|
    suffix = id.tr('-', '_')

    define_method("test_#{suffix}_original_is_red_and_solution_is_green") do
      task = available_task(id, 'red-green')
      assert_candidate(task, 'original-red', expected: false)
      assert_candidate(task, 'solution-green', expected: true, solution: true)
    end

    define_method("test_#{suffix}_every_editable_file_is_needed") do
      task = available_task(id, 'partial-edit')
      task.editable.each do |relative|
        assert_candidate(task, "missing-edit-#{relative}", expected: false, solution: true) do |workspace|
          FileUtils.cp(File.join(task.fixture_dir, relative), File.join(workspace, relative))
        end
      end
    end

    define_method("test_#{suffix}_rewriting_protected_tests_does_not_fix_the_bug") do
      task = available_task(id, 'tampered-tests')
      verdict = assert_candidate(task, 'tampered-tests', expected: false) do |workspace|
        Dir[File.join(workspace, 'test/**/*')].select { |path| File.file?(path) }.each do |path|
          # Simulate a model replacing the entry point and tests with successful exits.
          File.write(path, path.end_with?('.ts') ? 'process.exit(0);' : "exit 0\n")
        end
      end
      refute verdict[:verifier_passed], 'host must reconstruct protected tests from the fixture'
      refute verdict[:protected_intact]
    end

    define_method("test_#{suffix}_green_code_plus_protected_changes_is_still_rejected") do
      task = available_task(id, 'protected-change')
      verdict = assert_candidate(task, 'protected-change', expected: false, solution: true) do |workspace|
        protected_test = task.verify.first.last
        P0EvalContracts.write(workspace, protected_test, "exit 0\n")
      end
      assert verdict[:verifier_passed], 'clean reconstruction should use the original verifier and correct code'
      refute verdict[:protected_intact], 'driver must reject a candidate even when the clean verifier passes'
    end
  end

  def test_rust_constant_answer_and_broad_text_rename_are_rejected
    task = available_task(P0EvalContracts::IDS[0], 'rust-cheats')
    assert_candidate(task, 'constant-price', expected: false, solution: true) do |workspace|
      P0EvalContracts.write(workspace, 'src/pricing.rs', <<~RUST)
        pub fn quote_cents(_unit_cents: u64, _quantity: u32, _discount_bps: u16) -> u64 { 293 }
      RUST
    end
    assert_candidate(task, 'broad-text-rename', expected: false) do |workspace|
      task.editable.each do |relative|
        text = File.read(File.join(workspace, relative)).gsub(/\bquote\b/, 'quote_cents')
        P0EvalContracts.write(workspace, relative, text)
      end
    end
  end

  def test_typescript_constant_answer_and_text_only_renames_are_rejected
    task = available_task(P0EvalContracts::IDS[1], 'typescript-cheats')
    assert_candidate(task, 'constant-label', expected: false, solution: true) do |workspace|
      path = 'src/formatter.ts'
      text = File.read(File.join(workspace, path)).sub('return value.trim().toUpperCase();', 'return "ADA";')
      P0EvalContracts.write(workspace, path, text)
    end
    assert_candidate(task, 'broad-text-rename', expected: false) do |workspace|
      task.editable.each do |relative|
        text = File.read(File.join(workspace, relative)).gsub('formatLabel', 'renderLabel')
        P0EvalContracts.write(workspace, relative, text)
      end
    end
    assert_candidate(task, 'stale-string-key', expected: false, solution: true) do |workspace|
      path = 'src/view.ts'
      text = File.read(File.join(workspace, path)).sub('formatter["renderLabel"]', 'formatter["formatLabel"]')
      P0EvalContracts.write(workspace, path, text)
    end
    assert_candidate(task, 'unrelated-literal-renamed', expected: false, solution: true) do |workspace|
      path = 'src/view.ts'
      text = File.read(File.join(workspace, path)).sub('literal: "formatLabel"', 'literal: "renderLabel"')
      P0EvalContracts.write(workspace, path, text)
    end
  end

  def test_config_hardcoding_and_changing_protected_configuration_are_rejected
    task = available_task(P0EvalContracts::IDS[2], 'config-cheats')
    assert_candidate(task, 'hardcoded-config', expected: false, solution: true) do |workspace|
      path = 'lib/manifest.rb'
      text = File.read(File.join(workspace, path)).sub("config.fetch('max_batch_size')", '4')
      P0EvalContracts.write(workspace, path, text)
    end
    assert_candidate(task, 'constant-chunks', expected: false, solution: true) do |workspace|
      P0EvalContracts.write(workspace, 'lib/batching.rb', <<~RUBY)
        module Batching
          def self.chunks(items, limit:)
            [items.first(4), items.drop(4).first(4), items.drop(8).first(3)]
          end
        end
      RUBY
    end
    verdict = assert_candidate(task, 'config-workaround', expected: false, solution: true) do |workspace|
      FileUtils.cp(File.join(task.fixture_dir, 'lib/manifest.rb'), File.join(workspace, 'lib/manifest.rb'))
      P0EvalContracts.write(workspace, 'config/export.json', JSON.generate('schema_version' => 2, 'max_batch_size' => 3))
    end
    refute verdict[:verifier_passed]
    refute verdict[:protected_intact]
  end

  def test_unknown_new_files_are_protected_even_with_a_correct_solution
    task = available_task(P0EvalContracts::IDS[2], 'extra-file')
    verdict = assert_candidate(task, 'extra-file', expected: false, solution: true) do |workspace|
      P0EvalContracts.write(workspace, 'test_bypass.rb', "exit 0\n")
    end
    assert verdict[:verifier_passed]
    refute verdict[:protected_intact]
  end

  def test_offline_reports_cannot_contaminate_real_model_history
    bench = P0EvalContracts::BENCH
    assert_raises(ArgumentError) { P0EvalContracts.report_directory(bench) }
    assert_raises(ArgumentError) { P0EvalContracts.report_directory(File.join(bench, 'nested/new-report')) }
    assert_equal File.join(P0EvalContracts::ROOT, 'target/p0-eval-contracts'),
                 P0EvalContracts.report_directory(File.join(P0EvalContracts::ROOT, 'target/p0-eval-contracts'))
    Dir.mktmpdir('p0-report-path-check-') do |temporary|
      alias_path = File.join(temporary, 'bench-alias')
      File.symlink(bench, alias_path)
      assert_raises(ArgumentError) { P0EvalContracts.report_directory(File.join(alias_path, 'nested/report')) }
    end
  end

  def test_reports_reuse_the_existing_json_and_markdown_renderer_without_model_telemetry
    task = P0EvalContracts.task(P0EvalContracts::IDS[2])
    row = P0EvalContracts.row(task, 'report-example', status: 'passed')
    summary = ModelEval::Report.summarize(model: 'offline-fixture-contracts-no-model', rows: [row])
    assert_nil summary['tokens']
    assert_nil summary['narration_ratio']
    assert_equal 0, summary['turns']
    rendered = P0EvalContracts.offline_markdown({ 'rows' => [row.transform_keys(&:to_s)] }, summary)
    assert_includes rendered, '本报告没有运行模型'
    assert_includes rendered, '契约断言通过率'
    assert_includes rendered, 'passed = 契约断言符合预期'
    refute_includes rendered, 'passed = 外部验收通过且受保护文件未动'
    Dir.mktmpdir('p0-report-check-') do |output|
      json, markdown, history = ModelEval::Report.archive({ 'rows' => [row.transform_keys(&:to_s)] }, summary, output)
      assert_equal summary, JSON.parse(File.read(json)).fetch('summary')
      assert_includes File.read(markdown), 'offline-fixture-contracts-no-model'
      assert_equal 1, File.readlines(history).size
    end
  end
end
