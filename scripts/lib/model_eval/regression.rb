# frozen_string_literal: true

require_relative '../../agent_reliability_eval'
require_relative '../../model_eval'

module ModelEval
  # Fixed regression registration and orchestration only. Processes, model task
  # verifiers and report aggregation stay in the existing evaluation machinery.
  module Regression
    ROOT = ModelEval::ROOT
    MANIFEST = File.join(ModelEval::BENCH, 'core-regression.json')
    REQUIRED = %w[stale_edit cross_file_references browser_interaction compression_constraints multitask_recovery false_completion].freeze
    MAX_OUTPUT_BYTES = AgentEvalProcess::MAX_LOG_BYTES
    CARGO_SECONDS = 900
    CHECK_SECONDS = 180

    module_function

    def workspace_version
      File.read(File.join(ROOT, 'Cargo.toml'), encoding: 'UTF-8')[/\[workspace\.package\]\s*\nversion\s*=\s*"([^"]+)"/, 1] || raise('workspace version missing')
    end

    def repository_path(relative)
      raise ArgumentError, "invalid repository path: #{relative.inspect}" unless relative.is_a?(String) && !relative.empty?
      path = File.expand_path(relative, ROOT)
      raise ArgumentError, "path outside repository: #{relative}" unless path.start_with?(ROOT + File::SEPARATOR)
      raise ArgumentError, "fixture missing: #{relative}" unless File.file?(path)
      raise ArgumentError, "fixture symlink outside repository: #{relative}" unless File.realpath(path).start_with?(File.realpath(ROOT) + File::SEPARATOR)
      path
    end

    def load_manifest(path = MANIFEST)
      spec = JSON.parse(File.read(path, encoding: 'UTF-8'))
      raise ArgumentError, 'unsupported regression schema' unless spec['schema_version'] == 1
      cases = spec.fetch('cases')
      raise ArgumentError, 'the six original scenarios must be registered exactly once' unless cases.map { |row| row.fetch('id') }.sort == REQUIRED.sort
      names = []
      cases.each do |row|
        raise ArgumentError, "#{row['id']}: fixture and independent criteria required" if row.fetch('fixture').fetch('description').empty? || row.fetch('criteria').empty?
        row['fixture'].fetch('paths').each { |relative| repository_path(relative) }
        checks = row.fetch('checks')
        raise ArgumentError, "#{row['id']}: no checks" if checks.empty?
        checks.each do |check|
          name = check.fetch('name')
          raise ArgumentError, "#{row['id']}: invalid test name" unless name.is_a?(String) && name.match?(/\A[A-Za-z_][A-Za-z_0-9:]*\z/)
          identity = [check.fetch('type'), check['script'], name]
          raise ArgumentError, "duplicate regression check: #{name}" if names.include?(identity)
          names << identity
          case check['type']
          when 'rust'
            raise ArgumentError, 'only the existing core library test target is registered' unless check['package'] == 'willdeep-core' && check['target'] == 'lib'
          when 'minitest'
            repository_path(check.fetch('script'))
            raise ArgumentError, 'only scripts/test entrypoints are allowed' unless check['script'].start_with?('scripts/test/')
          when 'browser_runtime'
            raise ArgumentError, 'browser stage must use the fixed Runtime entrypoint' unless check['script'] == 'scripts/test/browser_runtime_entrypoints_test.rb' && check['stage'] == 'browser'
            expected = check.fetch('expected_tests')
            raise ArgumentError, 'browser stage requires distinct fixed test names' unless expected.is_a?(Array) && !expected.empty? && expected.all? { |item| item.is_a?(String) && !item.empty? } && expected.uniq == expected
          else
            raise ArgumentError, "unknown check type: #{check['type']}"
          end
        end
      end
      spec
    end

    def read_output(path)
      raise "test log exceeds capture limit: #{path}" if File.size(path) >= MAX_OUTPUT_BYTES
      File.binread(path).force_encoding('UTF-8').scrub
    end

    # --exact alone can exit 0 with zero tests; ignore can exit 0 without running.
    # Require the named terminal result AND a single nonignored test summary.
    def rust_verdict(text, name)
      records = text.scan(/^test ([A-Za-z_0-9:]+) \.\.\. (ok|FAILED|ignored(?:[^\n]*))$/)
      summaries = text.scan(/^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out;/)
      valid = records == [[name, 'ok']] && summaries.size == 1 && summaries[0][0, 5] == %w[ok 1 0 0 0]
      { passed: valid, actual_tests: summaries.size == 1 ? summaries[0][1].to_i + summaries[0][2].to_i : 0,
        ignored: summaries.size == 1 ? summaries[0][3].to_i : nil, observed_names: records.map(&:first) }
    end

    def minitest_verdict(text, name)
      records = text.scan(/^([A-Za-z_0-9:]+)#(test_[A-Za-z_0-9]+) = [0-9.]+ s = ([.FES])$/)
      summaries = text.scan(/^(\d+) runs, (\d+) assertions, (\d+) failures, (\d+) errors, (\d+) skips$/)
      counts = summaries.size == 1 ? summaries[0].map(&:to_i) : []
      valid = records.size == 1 && records[0][1..2] == [name, '.'] && counts.size == 5 && counts[0] == 1 && counts[1].positive? && counts[2..4] == [0, 0, 0]
      { passed: valid, actual_tests: counts[0] || 0, assertions: counts[1], ignored: counts[4],
        observed_names: records.map { |record| record[1] } }
    end

    # Cargo's fresh compiler-artifact event binds this executable to the current
    # package version. A random old test binary or a historical report is not input.
    def cargo_artifact(text, version)
      events = text.lines.map { |line| JSON.parse(line) }
      raise 'Cargo output must contain object records' unless events.all? { |event| event.is_a?(Hash) }
      raise 'Cargo did not finish a successful build' unless events.last == {'reason' => 'build-finished', 'success' => true}
      artifacts = events.select do |event|
        event['reason'] == 'compiler-artifact' && event.dig('profile', 'test') == true &&
          event.dig('target', 'name') == 'willdeep_core' && event.dig('target', 'kind') == ['lib'] && event['executable']
      end
      raise 'Cargo must report exactly one core library test executable' unless artifacts.size == 1
      artifact = artifacts.first
      raise 'Cargo test package version does not match workspace' unless artifact.fetch('package_id').match?(/(?:@|#)#{Regexp.escape(version)}\z/)
      expected_source = File.join(ROOT, 'crates/willdeep-core/src/lib.rs')
      raise 'Cargo test target is outside the current worktree' unless artifact.dig('target', 'src_path') == expected_source
      binary = artifact.fetch('executable')
      raise 'Cargo test executable missing' unless File.file?(binary) && File.executable?(binary)
      { path: binary, version: version, sha256: Digest::SHA256.file(binary).hexdigest, package_id: artifact['package_id'] }
    rescue JSON::ParserError => error
      raise "invalid Cargo artifact stream: #{error.message}"
    end

    def capture(command, seconds, directory)
      FileUtils.mkdir_p(directory, mode: 0o700)
      code, timeout, elapsed = AgentReliabilityEval.run_process(command,
        {'CARGO_NET_OFFLINE' => 'true', 'CARGO_TERM_COLOR' => 'never', 'NO_COLOR' => '1'}, ROOT, seconds, directory)
      { exit_code: code, timed_out: timeout, elapsed_seconds: elapsed.round(3), command: command,
        stdout: File.join(directory, 'stdout.log'), stderr: File.join(directory, 'stderr.log') }
    end

    def build_core(directory, version, seconds)
      command = ['cargo', 'test', '--offline', '--locked', '-p', 'willdeep-core', '--lib', '--no-run', '--message-format=json']
      execution = capture(command, seconds, directory)
      return { execution: execution, error: execution[:timed_out] ? 'Cargo build timed out' : 'Cargo build failed' } unless execution[:exit_code] == 0 && !execution[:timed_out]
      { execution: execution, artifact: cargo_artifact(read_output(execution[:stdout]), version) }
    rescue RuntimeError, KeyError, SystemCallError => error
      { execution: execution, error: error.message }
    end

    def browser_verdict(report, expected, version, started_at)
      return { passed: false, actual_tests: 0, observed_names: [] } unless report.is_a?(Hash) && report['cases'].is_a?(Array) && !expected.empty?
      tests = report.fetch('cases')
      return { passed: false, actual_tests: 0, observed_names: [] } unless tests.all? { |test| test.is_a?(Hash) && test['entry'].is_a?(String) && test['scenario'].is_a?(String) }
      names = tests.map { |test| "#{test.fetch('entry')}/#{test.fetch('scenario')}" }
      same_run = Time.iso8601(report.fetch('started_at')) >= started_at - 1
      binary = report.fetch('binary')
      binary_matches = binary.is_a?(String) && File.file?(binary) && Digest::SHA256.file(binary).hexdigest == report['binary_sha256']
      executed = tests.count { |test| test['seconds'].is_a?(Numeric) && test['seconds'] >= 0 }
      valid = report['suite'] == 'browser-runtime-entrypoints' && report['passed'] == true &&
        report['version'] == version && same_run && binary_matches && names.uniq == names &&
        names.sort == expected.sort && executed == expected.size && tests.all? { |test| test['status'] == 'passed' }
      { passed: valid, actual_tests: executed, observed_names: names, version: report['version'], same_run: same_run, binary_matches: binary_matches }
    rescue KeyError, ArgumentError, TypeError, SystemCallError
      { passed: false, actual_tests: 0, observed_names: [] }
    end

    def run_browser(check, directory, options)
      started_at = Time.now.utc
      execution = capture([RbConfig.ruby, repository_path(check.fetch('script'))] + options[:browser_args] + ['--report-dir', directory], options[:timeout], directory)
      path = File.join(directory, 'browser-runtime-entrypoints.json')
      report = File.file?(path) ? JSON.parse(File.read(path, encoding: 'UTF-8')) : {}
      verdict = browser_verdict(report, check.fetch('expected_tests'), workspace_version, started_at)
      status = if execution[:timed_out] then 'timeout'
               elsif execution[:exit_code] != 0 || !verdict[:passed] then 'failed'
               else 'passed'
               end
      { name: check['name'], status: status, actual_tests: verdict[:actual_tests], evidence: verdict, execution: execution, report: path }
    rescue ArgumentError, RuntimeError, KeyError, SystemCallError, JSON::ParserError => error
      { name: check['name'], status: 'error', actual_tests: 0, reason: error.message, execution: execution }
    end

    def run_check(check, directory, options, build)
      return run_browser(check, directory, options) if check['type'] == 'browser_runtime'
      if check['type'] == 'rust'
        return { name: check['name'], status: build.dig(:execution, :timed_out) ? 'timeout' : 'error', actual_tests: 0, reason: build[:error] } unless build[:artifact]
        artifact = build[:artifact]
        raise 'test executable changed after Cargo build' unless Digest::SHA256.file(artifact[:path]).hexdigest == artifact[:sha256]
        command = [artifact[:path], check['name'], '--exact', '--nocapture', '--test-threads=1', '--color=never']
      else
        command = [RbConfig.ruby, repository_path(check.fetch('script')), '--verbose', '--name', check['name']]
        command += options[:browser_args] if check['stage'] == 'browser'
      end
      execution = capture(command, options[:timeout], directory)
      verdict = if check['type'] == 'rust'
                  rust_verdict(read_output(execution[:stdout]), check['name'])
                else
                  minitest_verdict(read_output(execution[:stdout]), check['name'])
                end
      status = if execution[:timed_out] then 'timeout'
               elsif execution[:exit_code] != 0 || !verdict[:passed] then 'failed'
               else 'passed'
               end
      { name: check['name'], status: status, actual_tests: verdict[:actual_tests], evidence: verdict, execution: execution }
    rescue RuntimeError, KeyError, SystemCallError => error
      { name: check['name'], status: 'error', actual_tests: 0, reason: error.message, execution: execution }
    end

    def run_case(row, directory, options, build)
      checks = row.fetch('checks').each_with_index.map do |check, index|
        if check['stage'] == 'browser' && !options[:browser]
          { name: check['name'], status: 'error', actual_tests: 0, reason: 'browser stage not selected; use --with-browser with the existing local prerequisites' }
        else
          run_check(check, File.join(directory, index.to_s), options, build)
        end
      end
      status = %w[timeout error failed].find { |state| checks.any? { |check| check[:status] == state } } || 'passed'
      { task: row.fetch('id'), kind: 'test', language: 'runtime', split: 'regression', status: status,
        claimed: nil, false_completion: nil, missing: [], actual_tests: checks.sum { |check| check[:actual_tests] },
        elapsed_seconds: checks.sum { |check| check.dig(:execution, :elapsed_seconds).to_f }.round(3),
        checks: checks, fixture: row.fetch('fixture'), criteria: row.fetch('criteria') }
    end

    def markdown(report, summary)
      text = ModelEval::Report.markdown(report, summary)
      text = text.sub(/\A# 模型行为评测[^\n]*/, '# 六类核心场景离线回归')
      text = text.sub('**verifier 通过率**', '**确定性回归通过率**')
      text = text.sub(/^\| 误报完成率 \|.*$/, '| 模型误报完成率 | 未调用模型，未测量 |')
      text = text.sub(/^状态：passed =.*$/, '状态：passed = 固定测试名与真实非零执行结果均核实；failed = 断言失败或结果不符；timeout = 超时；error = 缺少执行证据。失败与超时日志保留。')
      lines = ['', "完整六类基线：#{report['baseline_complete'] ? '通过' : '未通过或未完整执行'}；工作区版本：#{report['version']}；源码稳定：#{report['source_stable']}",
        '本轮没有调用付费模型。Token 与模型费用未测量，JSON 为 null，不表示 0；确定性测试不能替代真实模型质量评测。', '',
        '| 场景 | 实际测试数 | 固定判据 |', '| --- | ---: | --- |']
      report.fetch('rows').each do |row|
        lines << "| #{row['task']} | #{row['actual_tests']} | #{row['criteria'].join('；')} |"
      end
      lines << ''
      report.fetch('rows').each do |row|
        row.fetch('checks').each do |check|
          execution = check[:execution]
          lines << "- `#{check[:name]}`：#{check[:status]}#{check[:reason] ? "（#{check[:reason]}）" : ''}#{execution ? "；stdout：`#{execution[:stdout]}`；stderr：`#{execution[:stderr]}`" : ''}"
        end
      end
      text + lines.join("\n") + "\n"
    end

    def safe_output(path)
      expanded = File.expand_path(path)
      current = expanded
      suffix = []
      until File.exist?(current)
        suffix.unshift(File.basename(current))
        current = File.dirname(current)
      end
      resolved = File.join(File.realpath(current), *suffix)
      root = File.realpath(ROOT)
      if resolved == root || (resolved.start_with?(root + File::SEPARATOR) && !resolved.start_with?(File.join(root, 'target') + File::SEPARATOR))
        raise ArgumentError, 'regression output inside the repository must be under target/, outside source and model history'
      end
      expanded
    end

    def fixture_digest(spec)
      files = spec.fetch('cases').flat_map { |row| row.fetch('fixture').fetch('paths') }.uniq.sort
      digest = Digest::SHA256.new
      files.each do |relative|
        path = repository_path(relative)
        value = if relative.start_with?('bench/model-eval/tasks/') && File.basename(path) == 'task.json'
                  ModelEval::Task.load(File.dirname(path)).content_sha256
                else
                  Digest::SHA256.file(path).hexdigest
                end
        digest << relative << "\0" << value << "\0"
      end
      digest.hexdigest
    end

    def run(spec, options)
      selected = spec.fetch('cases')
      if options[:only]
        unknown = options[:only] - REQUIRED
        raise ArgumentError, "unknown scenarios: #{unknown.join(', ')}" unless unknown.empty?
        selected = selected.select { |row| options[:only].include?(row['id']) }
      end
      raise ArgumentError, 'no scenarios selected' if selected.empty?
      out = safe_output(options[:out])
      FileUtils.mkdir_p(out, mode: 0o700)
      directory = Dir.mktmpdir('run-', out)
      version = workspace_version
      source = ModelEval::Report.source_digest(ROOT)
      fixtures = fixture_digest(spec)
      manifest_digest = Digest::SHA256.hexdigest(JSON.generate(spec))
      needs_rust = selected.any? { |row| row['checks'].any? { |check| check['type'] == 'rust' } }
      build = needs_rust ? build_core(File.join(directory, 'build'), version, options[:build_timeout]) : {}
      rows = selected.map { |row| run_case(row, File.join(directory, row['id']), options, build) }
      stable = version == workspace_version && source == ModelEval::Report.source_digest(ROOT)
      stable &&= manifest_digest == Digest::SHA256.hexdigest(JSON.generate(load_manifest(options[:manifest]))) && fixtures == fixture_digest(spec)
      rows.each { |row| row[:status] = 'error'; row[:stop_reason] = 'source changed during execution' } unless stable
      context = ModelEval::Report.git_context(ROOT)
      summary = ModelEval::Report.summarize(model: 'offline-core-regression-no-model', rows: rows, version: version, **context)
      summary['false_completions'] = summary['false_completion_rate'] = nil
      report = {'schema_version' => 1, 'evaluation_mode' => 'offline_core_regression', 'provider_mode' => 'not_called',
        'contains_model_runs' => false, 'model_cost' => nil, 'input_tokens' => nil, 'output_tokens' => nil,
        'version' => version, 'created_at' => Time.now.utc.iso8601, 'source_sha256' => source,
        'source_stable' => stable, 'fixture_sha256' => fixtures, 'manifest_sha256' => manifest_digest, 'required_scenarios' => REQUIRED,
        'pending_scenarios' => REQUIRED - rows.select { |row| row[:status] == 'passed' }.map { |row| row[:task] },
        'baseline_complete' => stable && rows.size == REQUIRED.size && rows.all? { |row| row[:status] == 'passed' },
        'build' => build, 'rows' => rows.map { |row| row.transform_keys(&:to_s) }, 'summary' => summary}
      AgentReliabilityEval.write(File.join(directory, 'report.json'), JSON.pretty_generate(report) + "\n")
      AgentReliabilityEval.write(File.join(directory, 'report.md'), markdown(report, summary))
      puts JSON.generate({report: File.join(directory, 'report.json'), markdown: File.join(directory, 'report.md'),
        baseline_complete: report['baseline_complete'], passed: summary['passed'], pending: report['pending_scenarios']})
      rows.all? { |row| row[:status] == 'passed' } && stable ? 0 : 1
    end

    def main(argv)
      options = {manifest: MANIFEST, out: File.join(ROOT, 'target/core-regression'), timeout: CHECK_SECONDS,
                 build_timeout: CARGO_SECONDS, browser: false, browser_args: []}
      OptionParser.new do |parser|
        parser.banner = 'Usage: ruby scripts/agent_reliability_regression.rb [options]'
        parser.on('--manifest PATH') { |value| options[:manifest] = File.expand_path(value) }
        parser.on('--out PATH') { |value| options[:out] = value }
        parser.on('--only LIST', 'Explicit subset; does not certify the six-case baseline') { |value| options[:only] = value.split(',') }
        parser.on('--timeout SECONDS', Integer) { |value| options[:timeout] = value }
        parser.on('--build-timeout SECONDS', Integer) { |value| options[:build_timeout] = value }
        parser.on('--with-browser', 'Run the existing real local browser stage; never install dependencies') { options[:browser] = true }
        parser.on('--browser-arg ARG', 'Pass one argument to the browser test entrypoint') { |value| options[:browser_args] << value }
        parser.on('--list') { options[:mode] = :list }
        parser.on('--check-manifest') { options[:mode] = :check }
      end.parse!(argv)
      raise ArgumentError, 'unexpected positional arguments' unless argv.empty?
      raise ArgumentError, 'timeouts must be positive' unless options[:timeout].positive? && options[:build_timeout].positive?
      spec = load_manifest(options[:manifest])
      if options[:mode]
        unavailable = spec['cases'].flat_map { |row| row['checks'] }.select do |check|
          check['stage'] == 'browser' && !File.file?(File.join(ROOT, check['script']))
        end.map { |check| check['script'] }.uniq
        puts JSON.pretty_generate({schema_version: 1, version: workspace_version, executed: false, baseline_complete: false,
          mode: options[:mode], unavailable_optional_stages: unavailable, cases: spec['cases']})
        return 0
      end
      run(spec, options)
    rescue ArgumentError, KeyError, JSON::ParserError, SystemCallError => error
      warn "core regression: #{error.message}"
      2
    end
  end
end
