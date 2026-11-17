#!/usr/bin/env ruby
# frozen_string_literal: true

# Real CLI + private PTY + Web Runtime HTTP/SSE acceptance. Start the companion
# support/browser_runtime_fixture.rb with preview_start first. No build/install,
# direct bridge execution, paid Provider, shared port, or real HOME is used here.
Encoding.default_external = Encoding::UTF_8
require 'optparse'
require_relative 'support/browser_runtime_support'

class BrowserRuntimeEntrypoints
  SCENARIOS = %w[success approval_cancel executing_cancel].freeze
  ENTRIES = %w[cli tui web].freeze

  def initialize(options)
    @options, @checks = options, []
    @report = {suite: 'browser-runtime-entrypoints', started_at: Time.now.utc.iso8601, cases: @checks,
      scope: 'real CLI run, real private TUI PTY, real Web /api/chat/stream and /api/turns/{id}/stop; shared Runtime browser',
      not_verified: ['Windows process jobs', 'OS-level whole-process network isolation', 'Web React rendering',
        'Cancellation during a partially written screenshot (this suite checks committed artifacts and pending navigation/input)']}
  end

  def run
    begin
      preflight
      ENTRIES.each { |entry| SCENARIOS.each { |scenario| check(entry, scenario) } }
    rescue StandardError => error
      @report[:setup_error] = {class: error.class.name, message: error.message}
      ENTRIES.each do |entry|
        SCENARIOS.each do |scenario|
          next if @checks.any? { |row| row[:entry] == entry && row[:scenario] == scenario }
          @checks << {entry: entry, scenario: scenario, status: 'failed', error: 'setup failed', evidence: @report[:setup_error]}
        end
      end
    ensure
      @report[:finished_at] = Time.now.utc.iso8601
      @report[:passed] = @checks.length == 9 && @checks.all? { |row| row[:status] == 'passed' }
      write_report
    end
    @report[:passed]
  end

  private

  def assert(value, message) = BrowserRuntime.assert(value, message)
  def api(operation, params = {}) = BrowserRuntime.api(@manifest, operation, params)
  def state(id) = BrowserRuntime.http(@manifest.fetch('fixture_origin'), "/__qa/state?id=#{id}")
  def release(id) = BrowserRuntime.http(@manifest.fetch('fixture_origin'), '/__qa/release', body: {id: id})

  def preflight
    @manifest = JSON.parse(File.read(@options.fetch(:fixture)))
    %w[binary node chrome].each do |key|
      supplied = @options.fetch(key.to_sym)
      assert(supplied.start_with?('/') && File.executable?(supplied), "explicit absolute executable required: #{key}")
      assert(File.realpath(supplied) == @manifest.fetch(key), "#{key} does not match the private preview fixture")
    end
    @root = File.realpath(@manifest.fetch('root'))
    assert(File.dirname(File.realpath(@options.fetch(:fixture))) == @root, 'manifest must belong to the private fixture root')
    env = @manifest.fetch('environment')
    %w[HOME USERPROFILE WILLDEEP_HOME XDG_CONFIG_HOME XDG_DATA_HOME XDG_CACHE_HOME TMPDIR TMP TEMP].each do |key|
      assert(File.expand_path(env.fetch(key)).start_with?(@root + '/'), "#{key} is outside private fixture root")
    end
    assert(env.fetch('SSH_CONNECTION').start_with?('127.0.0.1 '), 'OSC52-only private clipboard transport required')
    assert(Digest::SHA256.file(@manifest.fetch('binary')).hexdigest == @manifest.fetch('binary_sha256'), 'binary changed since fixture launch; restart fixture')
    assert(File.directory?(File.join(BrowserRuntime::ROOT, 'web/node_modules/@xterm/headless')), 'existing @xterm/headless required; do not install automatically')
    status = BrowserRuntime.command(env, @manifest.fetch('binary'), 'daemon', 'status')
    assert(status[/\bpid=(\d+)/, 1].to_i == @manifest.fetch('runtime_pid'), 'private Runtime PID changed')
    health = BrowserRuntime.http(@manifest.fetch('web_origin'), '/health')
    @report.merge!(version: @manifest.fetch('version').delete_prefix('willdeep '), binary: @manifest.fetch('binary'),
      binary_sha256: @manifest.fetch('binary_sha256'), node: @manifest.fetch('node'), chrome: @manifest.fetch('chrome'),
      root: @root, runtime_pid: @manifest.fetch('runtime_pid'), web_health: health,
      fixture_origin: @manifest.fetch('fixture_origin'), web_origin: @manifest.fetch('web_origin'))
  end

  def check(entry, scenario)
    id = "#{entry}-#{scenario.tr('_', '-')}-#{SecureRandom.hex(4)}"
    evidence = {id: id, timeline: [], control: {}}
    row = {entry: entry, scenario: scenario, status: 'running', evidence: evidence}
    @checks << row
    began = BrowserRuntime.now
    process = tui = stream = session_id = nil
    before_artifacts = artifact_files
    baseline_groups = browser_groups
    begin
      assert(baseline_groups.empty?, "previous case left owned browser processes: #{baseline_groups}")
      BrowserRuntime.http(@manifest.fetch('fixture_origin'), '/__qa/register', body: {id: id, scenario: scenario})
      # Session creation is setup, never Turn submission. The tested ingress below
      # is the only operation allowed to submit this case's prompt.
      session = api('session.create', workspace: @manifest.fetch('workspace'), profile: 'fixture', title: id)
      session_id = session.fetch('id')
      api('session.update_approval_mode', id: session_id, approval_mode: 'full_access')
      evidence[:session_id] = session_id
      prompt = "BROWSER_ENTRYPOINT #{id}"
      case entry
      when 'cli'
        process = BrowserRuntime::Command.new(@manifest.fetch('environment'), @manifest.fetch('binary'),
          'run', '--config', @manifest.fetch('config'), '--workspace', @manifest.fetch('workspace'),
          '--session', session_id, '--output', 'ndjson', prompt, directory: @manifest.fetch('workspace'))
        evidence[:control][:submission] = 'real binary run --session --output ndjson'
      when 'tui'
        tui = BrowserRuntime::Pty.new(@manifest, session_id)
        tui.submit(prompt)
        evidence[:control][:submission] = 'private PTY bracketed paste and Enter'
      when 'web'
        stream = BrowserRuntime::Stream.new(@manifest.fetch('web_origin'), '/api/chat/stream',
          {request_id: SecureRandom.uuid, session_id: session_id, workspace: @manifest.fetch('workspace'),
            prompt: prompt, language: 'en', approval_mode: 'full_access', approval_confirm: true})
        evidence[:control][:submission] = 'POST /api/chat/stream with production ChatRequest'
      end
      pump = tui && -> { tui.pump(0.03) }
      turn = BrowserRuntime.await('one actual Runtime Turn', pump: pump) do
        turns = api('turn.list', session_id: session_id)
        assert(turns.length <= 1, 'one ingress submitted duplicate Runtime Turns')
        if turns.empty? && process&.finished?
          raise "CLI exited before submitting a Runtime Turn: #{process.result}"
        end
        turns.first
      end
      evidence[:turn_id] = turn.fetch('id')
      milestone(evidence, 'Runtime Turn exists')
      gate_key = scenario == 'executing_cancel' ? 'hold_entered' : 'gate_entered'
      fixture = BrowserRuntime.await("#{entry} #{scenario} observable #{gate_key}", pump: pump) do
        current = state(id)
        assert(current.fetch('fixture_errors').empty?, "fixture errors: #{current['fixture_errors']}")
        unless current.fetch(gate_key)
          observed = api('turn.get', id: turn.fetch('id'))
          assert(!BrowserRuntime::TERMINAL.include?(observed.fetch('status')),
            "Runtime Turn ended as #{observed['status']} before #{gate_key}")
        end
        current if current.fetch(gate_key)
      end
      milestone(evidence, gate_key)
      evidence[:before] = fixture
      live_groups = browser_groups
      assert(live_groups.length == 1 && live_groups.first.fetch(:members).length >= 2,
        "expected one live owned Node/Chrome process group, got #{live_groups}")
      evidence[:processes_before] = live_groups
      private_profiles = profile_paths
      assert(!private_profiles.empty?, 'real Chrome private profile was not observed')
      evidence[:profiles_before] = private_profiles
      artifacts = fixture.fetch('receipts').filter_map { |receipt| receipt['artifact'] }
      assert(artifacts.length == 1, 'one committed screenshot receipt required before terminal control')
      evidence[:artifact_receipt] = verify_artifact(artifacts.first)
      assert((artifact_files - before_artifacts) == [File.realpath(artifacts.first.fetch('path'))], 'unreported artifact before terminal control')
      if scenario == 'success'
        assert(fixture.fetch('commits') == [{'value' => "Synthetic #{id}", 'receipt' => "Saved #{id}"}], 'form must commit exactly one synthetic business value')
        assert(fixture.fetch('receipts').any? { |receipt| receipt['action'] == 'assert' && receipt['passed'] == true }, 'no browser business assertion receipt')
        release(id)
        evidence[:control][:terminal_action] = 'release Provider only after business receipt and owned processes were observed'
      elsif scenario == 'approval_cancel'
        api('session.update_approval_mode', id: session_id, approval_mode: 'strict')
        release(id)
        approval = wait_approval(turn, pump)
        evidence[:approval] = approval
        milestone(evidence, 'exact fill approval pending')
        assert(approval.fetch('description').include?('browser Fill') && approval.fetch('description').include?('sha256:'), 'pending approval is not the exact browser fill')
        current_turn = api('turn.get', id: turn.fetch('id'))
        assert(current_turn.fetch('status') == 'waiting_approval', 'cancel did not start from waiting_approval')
        if tui
          tui.wait('exact browser approval visible in TUI') do |screen|
            text = screen.fetch('lines').join("\n")
            text.include?('browser Fill') && text.include?('Ctrl+X')
          end
          tui.write("\x18")
          evidence[:control][:terminal_action] = 'TUI Ctrl+X from exact approval dialog'
        else
          cancel(entry, session_id, turn, evidence)
        end
      else
        assert(api('turn.get', id: turn.fetch('id')).fetch('status') == 'running', 'held browser navigation was not executing')
        if tui
          # The shared stop shortcut is usable both inside and outside a gate.
          tui.write("\x18")
          evidence[:control][:terminal_action] = 'TUI Ctrl+X during held Chrome navigation'
        else
          cancel(entry, session_id, turn, evidence)
        end
      end
      wanted = scenario == 'success' ? 'completed' : 'cancelled'
      terminal = BrowserRuntime.await("#{entry} terminal #{wanted}", pump: pump) do
        value = api('turn.get', id: turn.fetch('id'))
        if BrowserRuntime::TERMINAL.include?(value.fetch('status'))
          assert(value.fetch('status') == wanted, "Runtime terminal #{value['status']}, expected #{wanted}")
          value
        end
      end
      evidence[:terminal_turn] = terminal
      milestone(evidence, "terminal #{wanted}")
      task = terminal['active_task_id'] && api('task.get', id: terminal['active_task_id'])
      evidence[:task] = task
      assert(task && task.fetch('origin_client').to_s.start_with?(entry + ':'), "Runtime task origin does not identify #{entry}: #{task && task['origin_client']}")
      BrowserRuntime.await('owned Node/Chrome process group and profile termination', seconds: 10, pump: pump) do
        table = BrowserRuntime.processes
        groups = live_groups.map { |group| group.fetch(:pgid) }
        members = BrowserRuntime.running(table.select { |item| groups.include?(item[:pgid]) })
        evidence[:processes_after] = members
        evidence[:profiles_after] = profile_paths
        members.empty? && evidence[:profiles_after].empty?
      end
      assert(browser_groups.empty?, 'Runtime leaked or replaced the browser process group')
      evidence[:artifact_after] = verify_artifact(artifacts.first)
      assert(evidence[:artifact_after] == evidence[:artifact_receipt], 'committed screenshot changed or disappeared during cancellation/close')
      assert((artifact_files - before_artifacts) == [File.realpath(artifacts.first.fetch('path'))], 'unexpected extra or partial artifact after terminal control')
      final_fixture = state(id)
      evidence[:after] = final_fixture
      assert(final_fixture.fetch('fixture_errors').empty?, "fixture errors: #{final_fixture['fixture_errors']}")
      if scenario != 'success'
        assert(final_fixture.fetch('commits').empty?, 'cancelled browser action committed business state')
        assert(final_fixture.fetch('step') == 4, 'cancelled action was retried or followed by another Provider request')
        assert(final_fixture.fetch('receipts').none? { |receipt| receipt['action'] == 'fill' || (receipt['action'] == 'open' && receipt['url'].to_s.end_with?('/hold')) }, 'pending action incorrectly produced a success receipt')
        pending = api('approval.list')
        assert(pending.none? { |value| value['task_id'] == terminal['active_task_id'] }, 'cancelled Turn left a pending approval')
        evidence[:approvals_after] = pending
        if scenario == 'executing_cancel'
          BrowserRuntime.await('Chrome held HTTP socket closed', seconds: 5, pump: pump) { state(id)['hold_closed'] }
          evidence[:held_navigation_socket_closed] = true
        end
      else
        assert(final_fixture.fetch('step') == 11, 'normal Runtime browser sequence did not finish')
        status = final_fixture.fetch('receipts').last
        assert(status['action'] == 'status' && status['runtime'] == 'not_started' && status['ready'] == false, 'browser status receipt remained live')
      end
      tools = BrowserRuntime.await('terminal Runtime tool ledger', seconds: 10, pump: pump) do
        values = api('tool.list', turn_id: turn.fetch('id'), limit: 100)
        evidence[:tools] = values
        values if !values.empty? && values.none? { |value| value['status'] == 'running' }
      end
      assert(tools.all? { |tool| tool['name'] == 'browser' }, 'unexpected tool bypassed the browser acceptance sequence')
      expected_statuses = scenario == 'success' ? {'completed' => 10} : {'completed' => 3, 'interrupted' => 1}
      assert(tools.group_by { |tool| tool.fetch('status') }.transform_values(&:length) == expected_statuses,
        "Runtime tool receipts do not match #{scenario}: #{tools}")
      if process
        evidence[:cli_result] = process.result(seconds: 10)
        if scenario == 'success'
          assert(evidence[:cli_result][:success] && evidence[:cli_result][:stdout].include?("BROWSER_ENTRYPOINT_COMPLETED #{id}"), 'CLI did not return successful final text')
        else
          assert(!evidence[:cli_result][:success], 'CLI reported success for cancelled or waiting Turn')
        end
      end
      if stream
        events = stream.finish
        evidence[:sse] = events
        submitted = events.find { |event| event['type'] == 'submitted' }
        assert(submitted && submitted['session_id'] == session_id && submitted['turn_id'] == turn['id'], 'Web SSE did not bind actual session and Turn')
        ending = events.last
        if scenario == 'success'
          assert(ending && ending['type'] == 'completed' && ending['text'].include?("BROWSER_ENTRYPOINT_COMPLETED #{id}"), 'Web SSE missing successful final event')
        else
          assert(ending && ending['type'] == 'error' && ending['message'] == 'turn.cancelled', 'Web SSE missing cancelled terminal receipt')
        end
      end
      if tui
        tui.wait('TUI terminal feedback') do |screen|
          text = screen.fetch('lines').join("\n")
          scenario == 'success' ? text.include?("BROWSER_ENTRYPOINT_COMPLETED #{id}") : text.match?(/cancelled|Cancelled|Interrupted|interrupt|stopped|Stopped/)
        end
        evidence[:tui_terminal] = tui.screen
      end
      row[:status] = 'passed'
    rescue StandardError => error
      row[:status] = 'failed'
      row[:error] = {class: error.class.name, message: error.message, backtrace: error.backtrace.first(6)}
    ensure
      row[:seconds] = (BrowserRuntime.now - began).round(3)
      capture_failure_evidence(evidence, id, session_id, tui, stream, process)
      cleanup_case(evidence, id, session_id)
      cleanup_resource(row, 'CLI child') { process&.close }
      cleanup_resource(row, 'Web SSE') { stream&.close }
      cleanup_resource(row, 'PTY evidence') { tui.snapshot(@options.fetch(:report_dir), id) if tui&.screen }
      cleanup_resource(row, 'PTY child') { tui&.close }
      File.write(File.join(@options.fetch(:report_dir), "#{id}.json"), JSON.pretty_generate(row) + "\n")
      puts JSON.generate(entry: entry, scenario: scenario, status: row[:status], error: row[:error])
    end
  end

  def milestone(evidence, label)
    evidence[:timeline] << {event: label, monotonic: BrowserRuntime.now.round(4), utc: Time.now.utc.iso8601(3)}
  end

  def cancel(entry, session, turn, evidence)
    if entry == 'web'
      path = "/api/turns/#{turn.fetch('id')}/stop"
      BrowserRuntime.http(@manifest.fetch('web_origin'), path, body: {}, expected: 202)
      evidence[:control][:terminal_action] = "POST #{path} HTTP 202"
    else
      text = BrowserRuntime.command(@manifest.fetch('environment'), @manifest.fetch('binary'), 'session', 'stop', session)
      evidence[:control][:terminal_action] = 'real binary session stop SESSION_ID'
      evidence[:control][:stop_receipt] = text
    end
    milestone(evidence, 'normal stop request accepted')
  end

  def wait_approval(turn, pump)
    BrowserRuntime.await('exact browser approval', pump: pump) do
      approvals = api('approval.list')
      approvals.find { |approval| approval['task_id'] == api('turn.get', id: turn.fetch('id'))['active_task_id'] }
    end
  end

  def browser_groups
    table = BrowserRuntime.processes
    descendants = BrowserRuntime.descendants(@manifest.fetch('runtime_pid'), table)
    leaders = descendants.select { |row| row[:ppid] == @manifest.fetch('runtime_pid') && row[:pid] == row[:pgid] }
    leaders.map do |leader|
      members = BrowserRuntime.running(table.select { |row| row[:pgid] == leader[:pgid] })
      {pgid: leader[:pgid], members: members}
    end.reject { |group| group[:members].empty? }
  end

  def artifact_files
    Dir.glob(File.join(@manifest.fetch('artifacts'), '*'), File::FNM_DOTMATCH)
      .select { |path| File.file?(path) }.map { |path| File.realpath(path) }.sort
  end

  def profile_paths
    Dir.glob(File.join(@manifest.fetch('environment').fetch('TMPDIR'), 'willdeep-browser-*', 'profile')).sort
  end

  def verify_artifact(artifact)
    path = File.realpath(artifact.fetch('path'))
    assert(path.start_with?(File.realpath(@manifest.fetch('artifacts')) + '/'), 'artifact escaped private configured directory')
    bytes = File.binread(path)
    assert(bytes.start_with?("\x89PNG\r\n\x1a\n".b), 'artifact is not PNG')
    width, height = bytes.byteslice(16, 8).unpack('NN')
    assert(artifact['mime'] == 'image/png' && artifact['width'] == width && artifact['height'] == height, 'PNG metadata mismatch')
    assert(artifact['bytes'] == bytes.bytesize && artifact['sha256'] == "sha256:#{Digest::SHA256.hexdigest(bytes)}", 'PNG content/hash/byte receipt mismatch')
    {path: path, mime: artifact['mime'], width: width, height: height, bytes: bytes.bytesize, sha256: artifact['sha256']}
  end

  def capture_failure_evidence(evidence, id, session, tui, stream, process)
    probes = {
      last_fixture: -> { state(id) }, last_process_groups: -> { browser_groups },
      last_profiles: -> { profile_paths }, last_artifact_files: -> { artifact_files }
    }
    probes[:last_turns] = -> { api('turn.list', session_id: session) } if session
    probes[:last_tools] = -> { api('tool.list', turn_id: evidence[:turn_id], limit: 100) } if evidence[:turn_id]
    probes[:sse] = -> { stream.snapshot } if stream
    probes[:last_tui_frame] = -> { tui.screen } if tui
    probes[:cli_result] = -> { process.result(seconds: 1) } if process&.finished?
    probes.each do |key, probe|
      evidence[key] = probe.call
    rescue StandardError => error
      (evidence[:capture_errors] ||= {})[key] = "#{error.class}: #{error.message}"
    end
  end

  def cleanup_resource(row, label)
    yield
  rescue StandardError => error
    row[:status] = 'failed'
    row[:error] ||= {class: error.class.name, message: "#{label}: #{error.message}"}
    (row[:evidence][:cleanup_errors] ||= []) << "#{label}: #{error.class}: #{error.message}"
  end

  def cleanup_case(evidence, id, session)
    release(id) # Release only this owned synthetic Provider gate, if still blocked.
    return unless session
    turns = api('turn.list', session_id: session)
    if turns.any? { |turn| !BrowserRuntime::TERMINAL.include?(turn['status']) }
      BrowserRuntime.command(@manifest.fetch('environment'), @manifest.fetch('binary'), 'session', 'stop', session)
    end
    BrowserRuntime.await('cleanup of owned browser descendants', seconds: 10) { browser_groups.empty? }
  rescue StandardError => error
    evidence[:cleanup_error] = "#{error.class}: #{error.message}"
    # Cleanup never turns failure into success; no manual process kill is used as
    # evidence of Runtime cancellation. fixture shutdown owns final containment.
    @checks.last[:status] = 'failed'
  end

  def write_report
    FileUtils.mkdir_p(@options.fetch(:report_dir))
    prefix = File.join(@options.fetch(:report_dir), 'browser-runtime-entrypoints')
    File.write(prefix + '.json', JSON.pretty_generate(@report) + "\n")
    lines = ['# Browser Runtime 三入口验收', '', "结果：#{@report[:passed] ? '通过' : '失败'}；九例必须全部通过，无 skip。", '',
      "版本：#{@report[:version] || '未取得'}", "二进制：#{@report[:binary] || @options[:binary]}", '',
      '| 入口 | 场景 | 结果 | 错误 |', '| --- | --- | --- | --- |']
    @checks.each do |row|
      error = row[:error].is_a?(Hash) ? row[:error][:message] : row[:error]
      lines << "| #{row[:entry]} | #{row[:scenario]} | #{row[:status]} | #{error.to_s.gsub('|', '\\|').gsub(/\s+/, ' ')} |"
    end
    lines.concat(['', '每例 JSON 包含终态、真实停止入口、Provider 工具回执、业务提交、PNG 元数据/哈希、进程组及私有 profile 清理证据。PTY 的 ANSI/帧 JSON 只用作操作过程证据，截图不代替业务断言。', '', '## 未验证边界', ''])
    lines.concat(@report[:not_verified].map { |value| "- #{value}" })
    File.write(prefix + '.md', lines.join("\n") + "\n")
    puts JSON.generate(passed: @report[:passed], cases: @checks.length, reports: [prefix + '.json', prefix + '.md'])
  end
end

if $PROGRAM_NAME == __FILE__
  options = {}
  OptionParser.new do |parser|
    parser.banner = 'ruby scripts/test/browser_runtime_entrypoints_test.rb --binary PATH --node PATH --chrome PATH --fixture PRIVATE_ROOT/manifest.json --report-dir PATH'
    %w[binary node chrome fixture report-dir].each do |key|
      parser.on("--#{key} PATH") { |value| options[key.tr('-', '_').to_sym] = value }
    end
  end.parse!
  %i[binary node chrome fixture report_dir].each { |key| options.fetch(key) }
  options[:report_dir] = File.expand_path(options[:report_dir])
  FileUtils.mkdir_p(options[:report_dir])
  exit(BrowserRuntimeEntrypoints.new(options).run ? 0 : 1)
end
