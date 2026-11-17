#!/usr/bin/env ruby
# frozen_string_literal: true

require 'minitest/autorun'
require 'tmpdir'
require_relative '../lib/model_eval/regression'

class AgentReliabilityRegressionTest < Minitest::Test
  Regression = ModelEval::Regression

  def rust_output(name, status: 'ok', passed: 1, failed: 0, ignored: 0)
    "\nrunning 1 test\ntest #{name} ... #{status}\n\ntest result: #{failed.zero? ? 'ok' : 'FAILED'}. #{passed} passed; #{failed} failed; #{ignored} ignored; 0 measured; 29 filtered out; finished in 0.01s\n"
  end

  def test_rust_requires_the_exact_test_and_nonignored_execution
    name = 'tools::tests::fixture'
    verdict = Regression.rust_verdict(rust_output(name), name)
    assert verdict[:passed]
    assert_equal 1, verdict[:actual_tests]
    assert_equal [name], verdict[:observed_names]
    refute Regression.rust_verdict(rust_output('tools::tests::other'), name)[:passed]
    refute Regression.rust_verdict(rust_output(name, status: 'ignored', passed: 0, ignored: 1), name)[:passed]
    refute Regression.rust_verdict(rust_output(name, status: 'FAILED', passed: 0, failed: 1), name)[:passed]
    refute Regression.rust_verdict("running 0 tests\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out;\n", name)[:passed]
    refute Regression.rust_verdict(rust_output(name) * 2, name)[:passed]
    refute Regression.rust_verdict('file_exists=true', name)[:passed]
  end

  def test_real_minitest_subprocess_and_assertions_are_observed
    Dir.mktmpdir('regression-minitest-') do |root|
      script = File.join(root, 'fixture_test.rb')
      marker = File.join(root, 'marker')
      File.write(script, <<~RUBY)
        require 'minitest/autorun'
        class LocalBehaviorTest < Minitest::Test
          def test_writes_and_reads_an_isolated_file
            File.write(#{marker.inspect}, 'observed')
            assert_equal 'observed', File.read(#{marker.inspect})
          end
        end
      RUBY
      execution = Regression.capture([RbConfig.ruby, script, '--verbose', '--name', 'test_writes_and_reads_an_isolated_file'], 10, File.join(root, 'logs'))
      assert_equal 0, execution[:exit_code]
      refute execution[:timed_out]
      assert_equal 'observed', File.read(marker)
      verdict = Regression.minitest_verdict(Regression.read_output(execution[:stdout]), 'test_writes_and_reads_an_isolated_file')
      assert verdict[:passed], File.read(execution[:stdout])
      assert_equal 1, verdict[:actual_tests]
      assert_equal 1, verdict[:assertions]
    end
  end

  def test_minitest_rejects_zero_runs_skip_no_assertions_wrong_name_and_multiple_summaries
    name = 'test_behavior'
    text = "LocalTest##{name} = 0.01 s = .\n1 runs, 1 assertions, 0 failures, 0 errors, 0 skips\n"
    assert Regression.minitest_verdict(text, name)[:passed]
    [text.sub('1 runs', '0 runs'), text.sub('1 assertions', '0 assertions'),
     text.sub('0 skips', '1 skips'), text.sub('0 errors', '1 errors'),
     text.sub('0 failures', '1 failures'), text.sub(' = .', ' = S'), text * 2,
     text.sub(name, 'test_unrelated')].each do |invalid|
      refute Regression.minitest_verdict(invalid, name)[:passed], invalid
    end
  end

  def artifact_stream(binary, version)
    [
      {reason: 'compiler-artifact', package_id: "path+file:///fixture#willdeep-core@#{version}",
       profile: {test: true}, target: {name: 'willdeep_core', kind: ['lib'], src_path: File.join(Regression::ROOT, 'crates/willdeep-core/src/lib.rs')}, executable: binary},
      {reason: 'build-finished', success: true}
    ].map { |row| JSON.generate(row) + "\n" }.join
  end

  def test_cargo_receipt_binds_test_target_version_and_executable_hash
    version = Regression.workspace_version
    text = artifact_stream(RbConfig.ruby, version)
    artifact = Regression.cargo_artifact(text, version)
    assert_equal version, artifact[:version]
    assert_equal Digest::SHA256.file(RbConfig.ruby).hexdigest, artifact[:sha256]
    assert_raises(RuntimeError) { Regression.cargo_artifact(text, '0.0.0-rc999') }
    assert_raises(RuntimeError) { Regression.cargo_artifact(text.sub('"test":true', '"test":false'), version) }
    assert_raises(RuntimeError) { Regression.cargo_artifact(text.sub('"success":true', '"success":false'), version) }
    assert_raises(RuntimeError) { Regression.cargo_artifact(text.sub('willdeep_core', 'unrelated'), version) }
    assert_raises(RuntimeError) { Regression.cargo_artifact(text * 2, version) }
    assert_raises(RuntimeError) { Regression.cargo_artifact('{broken', version) }
    assert_raises(RuntimeError) { Regression.cargo_artifact(artifact_stream('/missing/test-binary', version), version) }
  end

  def test_real_nonzero_exit_and_timeout_are_retained_in_private_logs
    Dir.mktmpdir('regression-process-') do |root|
      failed = Regression.capture([RbConfig.ruby, '-e', 'STDOUT.write("observed failure"); STDERR.write("reason"); exit 7'], 10, File.join(root, 'failed'))
      assert_equal 7, failed[:exit_code]
      refute failed[:timed_out]
      assert_equal 'observed failure', File.read(failed[:stdout])
      assert_equal 'reason', File.read(failed[:stderr])
      timed = Regression.capture([RbConfig.ruby, '-e', 'reader, writer = IO.pipe; reader.read'], 0.2, File.join(root, 'timeout'))
      assert timed[:timed_out]
      assert_nil timed[:exit_code]
      assert File.file?(timed[:stdout])
      assert File.file?(timed[:stderr])
    end
  end

  def test_all_six_scenarios_have_fixed_checks_fixtures_and_independent_criteria
    spec = Regression.load_manifest
    assert_equal Regression::REQUIRED.sort, spec['cases'].map { |row| row['id'] }.sort
    spec['cases'].each do |row|
      refute_empty row['checks']
      refute_empty row['fixture']['paths']
      refute_empty row['criteria']
    end
    browser = spec['cases'].find { |row| row['id'] == 'browser_interaction' }
    assert browser['checks'].all? { |check| check['stage'] == 'browser' }
    cross = spec['cases'].find { |row| row['id'] == 'cross_file_references' }
    assert cross['checks'].any? { |check| check['script'] == 'scripts/test/p0_eval_tasks_test.rb' }
  end

  def test_empty_or_duplicate_scenarios_cannot_pass_manifest_validation
    spec = Regression.load_manifest
    Dir.mktmpdir('regression-manifest-') do |root|
      path = File.join(root, 'manifest.json')
      altered = Marshal.load(Marshal.dump(spec))
      altered['cases'][0]['checks'] = []
      File.write(path, JSON.generate(altered))
      assert_raises(ArgumentError) { Regression.load_manifest(path) }
      altered = Marshal.load(Marshal.dump(spec))
      altered['cases'] << altered['cases'][0]
      File.write(path, JSON.generate(altered))
      assert_raises(ArgumentError) { Regression.load_manifest(path) }
      altered = Marshal.load(Marshal.dump(spec))
      altered['cases'][0]['checks'] << altered['cases'][0]['checks'][0]
      File.write(path, JSON.generate(altered))
      assert_raises(ArgumentError) { Regression.load_manifest(path) }
    end
  end

  def test_missing_browser_stage_cannot_be_counted_as_passed
    row = Regression.load_manifest['cases'].find { |item| item['id'] == 'browser_interaction' }
    result = Regression.run_case(row, '/unused-browser-stage', {browser: false}, {})
    assert_equal 'error', result[:status]
    assert_equal 0, result[:actual_tests]
    assert result[:checks].all? { |check| check[:status] == 'error' && check[:reason].include?('--with-browser') }
  end

  def test_browser_report_requires_fixed_names_current_version_and_this_run
    expected = %w[cli/success tui/success web/success]
    now = Time.now.utc
    version = Regression.workspace_version
    report = {'suite' => 'browser-runtime-entrypoints', 'version' => version, 'started_at' => now.iso8601,
      'binary' => RbConfig.ruby, 'binary_sha256' => Digest::SHA256.file(RbConfig.ruby).hexdigest, 'passed' => true,
      'cases' => expected.map { |name| entry, scenario = name.split('/'); {'entry' => entry, 'scenario' => scenario, 'status' => 'passed', 'seconds' => 0.1} }}
    assert Regression.browser_verdict(report, expected, version, now)[:passed]
    [report.merge('version' => '0.0.0-rc1'), report.merge('started_at' => (now - 60).iso8601),
     report.merge('cases' => []), report.merge('cases' => report['cases'].first(2)),
     report.merge('cases' => report['cases'] + report['cases']),
     report.merge('cases' => report['cases'].map { |row| row.merge('status' => 'skipped') }),
     report.merge('cases' => report['cases'].map { |row| row.reject { |key, _| key == 'seconds' } }),
     report.merge('binary_sha256' => 'outdated'), report.merge('passed' => false),
     {'cases' => nil}, [], nil].each do |invalid|
      refute Regression.browser_verdict(invalid, expected, version, now)[:passed], invalid.inspect
    end
  end

  def test_explicit_browser_stage_records_a_real_missing_fixture_failure
    Dir.mktmpdir('regression-browser-failure-') do |root|
      runner = File.join(Regression::ROOT, 'scripts/agent_reliability_regression.rb')
      command = [RbConfig.ruby, runner, '--only', 'browser_interaction', '--with-browser', '--out', File.join(root, 'report')]
      %w[binary node chrome].each { |key| command += ["--browser-arg=--#{key}", "--browser-arg=#{RbConfig.ruby}"] }
      command += ['--browser-arg=--fixture', "--browser-arg=#{File.join(root, 'missing-fixture.json')}"]
      execution = Regression.capture(command, 30, File.join(root, 'runner-logs'))
      assert_equal 1, execution[:exit_code], Regression.read_output(execution[:stderr])
      refute execution[:timed_out]
      receipt = JSON.parse(Regression.read_output(execution[:stdout]))
      report = JSON.parse(File.read(receipt.fetch('report'), encoding: 'UTF-8'))
      refute report['baseline_complete']
      assert_nil report['model_cost']
      assert_equal 0, report['rows'].first.fetch('actual_tests')
      check = report['rows'].first.fetch('checks').first
      refute_equal 'passed', check['status']
      browser = JSON.parse(File.read(check.fetch('report'), encoding: 'UTF-8'))
      assert_equal 'Errno::ENOENT', browser.fetch('setup_error').fetch('class')
      assert_equal 9, browser.fetch('cases').size
      assert browser['cases'].all? { |row| row['status'] == 'failed' }
      assert File.file?(receipt.fetch('markdown'))
    end
  end

  def test_build_failure_and_timeout_prevent_any_rust_pass
    check = {'type' => 'rust', 'name' => 'test_fixture'}
    failed = Regression.run_check(check, '/unused-build-failed', {}, {error: 'build failed', execution: {timed_out: false}})
    assert_equal 'error', failed[:status]
    assert_equal 0, failed[:actual_tests]
    timed = Regression.run_check(check, '/unused-build-timeout', {}, {error: 'build timed out', execution: {timed_out: true}})
    assert_equal 'timeout', timed[:status]
    assert_equal 0, timed[:actual_tests]
  end

  def test_outputs_cannot_enter_model_history_or_source_through_symlinks
    assert_raises(ArgumentError) { Regression.safe_output(ModelEval::BENCH) }
    assert_raises(ArgumentError) { Regression.safe_output(File.join(Regression::ROOT, 'scripts/generated')) }
    assert_equal File.join(Regression::ROOT, 'target/core-regression'), Regression.safe_output(File.join(Regression::ROOT, 'target/core-regression'))
    Dir.mktmpdir('regression-symlink-') do |root|
      File.symlink(ModelEval::BENCH, File.join(root, 'alias'))
      assert_raises(ArgumentError) { Regression.safe_output(File.join(root, 'alias/new-report')) }
    end
    assert_raises(ArgumentError) { Regression.repository_path('../private') }
  end

  def test_report_reuses_existing_metrics_without_claiming_model_measurements
    row = {task: 'false_completion', kind: 'test', status: 'passed', actual_tests: 1, claimed: nil, checks: [], criteria: ['blocked unchecked completion']}
    summary = ModelEval::Report.summarize(model: 'offline-core-regression-no-model', rows: [row])
    summary['false_completion_rate'] = nil
    report = {'rows' => [row.transform_keys(&:to_s)], 'baseline_complete' => false, 'version' => Regression.workspace_version, 'source_stable' => true}
    text = Regression.markdown(report, summary)
    assert_includes text, '确定性回归通过率'
    assert_includes text, '完整六类基线：未通过或未完整执行'
    assert_includes text, '未调用模型，未测量'
    refute_includes text, 'passed = 外部验收通过且受保护文件未动'
    assert_nil summary['tokens']
  end
end
