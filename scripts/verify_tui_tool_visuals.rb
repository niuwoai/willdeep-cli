#!/usr/bin/env ruby
# frozen_string_literal: true

# Real Runtime + PTY acceptance; no real model, user HOME, clipboard or Git.
# Usage: ruby scripts/verify_tui_tool_visuals.rb BINARY OUTPUT_DIRECTORY
# OUTPUT_DIRECTORY must be new or empty. Failed fixtures/evidence are retained.
require 'tmpdir'
require_relative 'test/support/tui_tool_visuals_support'

class ToolVisualRun
  A = ToolVisualAcceptance

  def initialize(binary, output)
    @binary, @output = binary, output
    @root = Dir.mktmpdir('wd-tui-tool-visuals-')
    @report = {passed: false, checks: [], errors: [], cleanup: [],
      binary: binary, binary_sha256_before: Digest::SHA256.file(binary).hexdigest,
      fixture_root: @root, real_model_requests: false, layouts: %w[full compact],
      scope: 'real Runtime/full+compact PTY; gated command expansion/collapse; committed create/edit/file_changes diffs; historical AI/time/divider adjacency',
      isolation: 'Private HOME/WILLDEEP_HOME/XDG/temp/workspace; allowlisted child environment; synthetic loopback Provider; file-lock synchronization; OSC52 captured in owned PTY.',
      boundary: 'No final finish after writes: the last Provider request remains gated until private Runtime shutdown. Does not verify real models, public-event redaction, Web/mobile, manual expansion, parallel calls or cross-platform behavior.'}
  end

  def run
    begin
      %w[full compact].each { |mode| run_mode(mode) }
    rescue StandardError, Interrupt => error
      @report[:errors] << "#{error.class}: #{error.message}"
    ensure
      begin
        @report[:binary_sha256_after] = Digest::SHA256.file(@binary).hexdigest
        unchanged = @report[:binary_sha256_after] == @report[:binary_sha256_before]
        @report[:checks] << {mode: 'binary', name: 'binary SHA256 unchanged', passed: unchanged}
        @report[:errors] << 'binary changed during acceptance' unless unchanged
      rescue StandardError => error
        @report[:errors] << "binary SHA256 after acceptance: #{error.class}: #{error.message}"
      end
      @report[:passed] = @report[:errors].empty? && @report[:checks].all? { |check| check[:passed] } &&
        @report[:cleanup].all? { |item| item[:passed] }
      @report[:checks_passed] = @report[:checks].count { |check| check[:passed] }
      @report[:fixture_retained] = !@report[:passed]
      if @report[:passed]
        begin
          FileUtils.remove_entry(@root)
        rescue StandardError => error
          @report[:passed] = false
          @report[:fixture_retained] = true
          @report[:errors] << "remove private fixture: #{error.class}: #{error.message}"
        end
      end
      write_reports
      puts "TUI tool visuals: #{@report[:passed] ? 'PASS' : 'FAIL'}; #{@report[:checks_passed]} checks passed"
      puts "Report: #{File.join(@output, 'tool-visuals.json')}"
      warn "Failed fixture retained: #{@root}" unless @report[:passed]
    end
    @report[:passed]
  end

  private

  def check(mode, name)
    detail = yield
    @report[:checks] << {mode: mode, name: name, passed: true, evidence: detail}
    puts "#{mode}: #{name}: PASS"
    detail
  rescue StandardError => error
    @report[:checks] << {mode: mode, name: name, passed: false, error: "#{error.class}: #{error.message}"}
    raise
  end

  def snapshot(tui, directory, name)
    tui.snapshot(directory, name)
    {screen: File.join(directory, "#{name}.json"), ansi: File.join(directory, "#{name}.ansi"),
      application_frame: tui.screen.fetch('application_frame')}
  end

  def receipt(request, id)
    matches = request.fetch(:messages).select { |message| message['role'] == 'tool' && message['tool_call_id'] == id }
    A.assert(matches.length == 1, "missing/duplicate Runtime tool receipt: #{id}")
    matches.first.fetch('content')
  end

  def completed_command_summaries(screen)
    visible = A.chat(screen)
    # A single completed command keeps its own row; consecutive tools fold into
    # the existing aggregate. Accept only aggregates without a failure suffix.
    individual = visible.lines.count { |line| line.include?('✓') && line.include?('run_command') }
    folded = visible.scan(/\b(\d+) tool calls folded · \/tools to expand\b/).sum { |count| count.first.to_i }
    individual + folded
  end

  def collapsed_commands(screen, gates)
    gates.each { |gate| A.assert(!A.has_tail?(screen, gate.tail), "completed command is still expanded: #{gate.tail}") }
    A.assert(completed_command_summaries(screen) >= gates.length, 'completed command summaries are missing from captured chat')
  end

  def wait_diff(tui, path, added, removed)
    tui.wait("committed diff for #{path}", seconds: A::WAIT_SECONDS) do |screen|
      A.chat(screen).match?(/#{Regexp.escape(path)}\s+\+#{added}\s+[−-]#{removed}(?!\d)/)
    end
  end

  def run_mode(mode)
    directory = File.join(@output, mode)
    FileUtils.mkdir_p(directory, mode: 0o700)
    private_root = File.join(@root, mode)
    environment = A.environment(private_root)
    workspace = File.join(private_root, 'workspace')
    FileUtils.mkdir_p(workspace, mode: 0o700)
    initial_edit = "EDIT_CONTEXT_BEFORE\nEDIT_OLD_LINE\nEDIT_CONTEXT_AFTER\n"
    initial_batch = "BATCH_CONTEXT_BEFORE\nBATCH_OLD_LINE\nBATCH_CONTEXT_AFTER\n"
    File.write(File.join(workspace, 'visual-edit.txt'), initial_edit)
    File.write(File.join(workspace, 'visual-batch.txt'), initial_batch)
    gates, provider, tui, config, started_runtime = [], nil, nil, nil, false
    provider = ToolVisualProvider.new
    config, session = A.configure(environment, workspace, mode, provider)
    @report[:version] ||= interaction_command(environment, @binary, '--version').strip
    # An explicitly registered private workspace avoids the first-open approval
    # modal. This affects only the fresh Runtime under this fixture's HOME.
    started_runtime = true
    interaction_command(environment, @binary, '--config', config, 'daemon', 'start')
    A.api(environment, @binary, config, 'workspace.register', root: workspace,
      name: 'Private TUI tool visual acceptance', access: 'full_access')
    A.api(environment, @binary, config, 'workspace.set_approval_mode', root: workspace,
      approval_mode: 'full_access', confirm_full_access: true)
    File.write(File.join(directory, 'runtime-status.txt'),
      interaction_command(environment, @binary, '--config', config, 'daemon', 'status'))
    tui = ToolVisualPty.new(@binary, environment, config, workspace, session)
    tui.wait('private resumed session') { |screen| A.chat(screen).include?('TOOL_VISUALS_READY') }
    check(mode, 'requested layout is painted through the default Runtime TUI') do
      activity = tui.screen.fetch('lines').any? { |line| line.start_with?('┌Activity') }
      A.assert(activity == (mode == 'full'), "configured #{mode} layout does not match the painted frame")
      snapshot(tui, directory, '00-layout-ready').merge(columns: tui.columns, rows: tui.rows, activity_panel: activity)
    end

    # Finish a text-only real Runtime turn before writes exist. Its historical
    # ending can then be observed while the next Provider response is held open.
    A.submit(tui, provider, 'HISTORY_VISUAL_REQUEST', 1)
    provider.delta(0, "HISTORY_AI_FIRST_LINE\nHISTORY_AI_LAST_LINE\n")
    provider.finish(0)
    tui.wait('first Runtime turn finished') do |screen|
      A.chat(screen).include?('HISTORY_AI_LAST_LINE') && A.chat(screen).include?('turn finished')
    end
    snapshot(tui, directory, '00-finished-turn')
    A.submit(tui, provider, 'ACTIVE_VISUAL_TOOLS_REQUEST', 2)
    tui.wait('historical your-turn prompt removed') do |screen|
      divider = A.chat_lines(screen).find { |row| row.include?('turn finished') }
      A.chat(screen).include?('ACTIVE_VISUAL_TOOLS_REQUEST') && divider && !divider.match?(/your turn/i)
    end
    check(mode, 'historical ending touches AI/time and hides your turn') do
      evidence = A.assert_history(tui.screen)
      evidence.merge(snapshot(tui, directory, '01-historical-ending'))
    end

    index = 1
    2.times do |offset|
      gate = ToolVisualCommandGate.new(workspace, offset + 1)
      gates << gate
      id = "visual-command-#{offset + 1}"
      provider.reasoning(index, offset.zero? ? 'EARLIER_THOUGHT_BODY' : 'CLICKED_LATEST_THOUGHT_BODY')
      provider.tool(index, id: id, name: 'run_command', arguments: {
        command: gate.command, timeout_seconds: 120, run_in_background: false})
      tui.wait("command #{offset + 1} reached private lock", seconds: A::WAIT_SECONDS) do |screen|
        provider.assert_healthy!
        File.file?(gate.started) && !File.exist?(gate.finished) && A.has_tail?(screen, gate.tail)
      end
      check(mode, "running command #{offset + 1} expands past the 96-character summary") do
        A.assert(File.read(gate.started) == 'ready', 'real shell child did not reach its gate')
        A.assert(!File.exist?(gate.finished), 'command completed before the running capture')
        A.assert(provider.requests.length == index + 1, 'next Provider request started before gate release')
        A.assert(A.chat(tui.screen).include?('run_command'), 'running tool row absent')
        gates[0...-1].each { |old| A.assert(!A.has_tail?(tui.screen, old.tail), 'historical command auto-expanded') }
        snapshot(tui, directory, "02-command-#{offset + 1}-running").merge(
          command: gate.command, tail: gate.tail, tail_character_offset: gate.command.index(gate.tail))
      end
      gate.release
      following = A.request(tui, provider, index + 2)
      result = receipt(following, id)
      tui.wait("command #{offset + 1} returns to summary", seconds: A::WAIT_SECONDS) do |screen|
        !A.has_tail?(screen, gate.tail) && completed_command_summaries(screen) >= gates.length
      end
      check(mode, "completed command #{offset + 1} collapses while next Provider waits") do
        A.assert(File.read(gate.finished) == 'complete', 'real command did not complete')
        A.assert(result.match?(/\Aexit_code: 0\b/) && result.include?(gate.output_marker), 'command receipt does not prove successful execution')
        collapsed_commands(tui.screen, gates)
        A.assert(provider.requests.length == index + 2, 'Provider did not remain at the controlled next request')
        snapshot(tui, directory, "03-command-#{offset + 1}-collapsed")
      end
      index += 1
    end

    check(mode, 'clicking the last folded thought reveals only its own body') do
      headers = A.chat_lines(tui.screen).each_index.select { |row| A.chat_lines(tui.screen)[row].include?('+ Thought:') }
      A.assert(headers.length == 1, 'two tool rounds must fold into one thought header')
      A.assert(!A.chat(tui.screen).include?('EARLIER_THOUGHT_BODY'), 'earlier thought body was already visible')
      tui.click(4, headers.fetch(0))
      tui.wait('latest thought expands without revealing the earlier one') do |screen|
        A.chat(screen).include?('CLICKED_LATEST_THOUGHT_BODY')
      end
      A.assert(!A.chat(tui.screen).include?('EARLIER_THOUGHT_BODY'), 'click expanded the wrong earlier thought')
      A.assert(A.chat_lines(tui.screen).count { |row| row.match?(/[+−] Thought:/) } == 1, 'click created a second thought header')
      expanded = snapshot(tui, directory, '03-thought-expanded')
      header = A.chat_lines(tui.screen).index { |row| row.include?('− Thought:') }
      tui.click(4, header)
      tui.wait('clicked thought collapses again') do |screen|
        A.chat(screen).include?('+ Thought:') && !A.chat(screen).include?('CLICKED_LATEST_THOUGHT_BODY')
      end
      collapsed = snapshot(tui, directory, '03-thought-collapsed')
      tui.wheel(up: false, count: 100)
      {expanded: expanded, collapsed: collapsed}
    end

    created = "CREATE_FIRST_LINE\nCREATE_MIDDLE_LINE\nCREATE_LAST_LINE\n"
    check(mode, 'create_file commits a green numbered diff with filename/counts') do
      result = A.tool(tui, provider, index, 'visual-create', 'create_file', path: 'visual-create.txt', content: created)
      index += 1
      A.assert(JSON.parse(result).fetch('status') == 'created', 'create_file did not return a committed receipt')
      A.assert(File.read(File.join(workspace, 'visual-create.txt')) == created, 'create_file disk bytes differ')
      wait_diff(tui, 'visual-create.txt', 3, 0)
      detail = A.assert_diff(tui.screen, 'visual-create.txt', added: 3, removed: 0, rows: [
        {old: nil, new: 1, body: '+CREATE_FIRST_LINE', color: :green},
        {old: nil, new: 2, body: '+CREATE_MIDDLE_LINE', color: :green},
        {old: nil, new: 3, body: '+CREATE_LAST_LINE', color: :green}])
      detail.merge(snapshot(tui, directory, '04-create-diff'))
    end

    check(mode, 'edit_file reads a real baseline then commits red/green/context rows') do
      baseline = A.tool(tui, provider, index, 'visual-read-edit', 'read_file', path: 'visual-edit.txt')
      index += 1
      A.assert(baseline.include?('EDIT_OLD_LINE') && baseline.match?(/\[version: sha256:[0-9a-f]{64}\]/), 'read_file baseline/version is absent')
      result = A.tool(tui, provider, index, 'visual-edit', 'edit_file', path: 'visual-edit.txt',
        old_string: 'EDIT_OLD_LINE', new_string: 'EDIT_NEW_LINE')
      index += 1
      A.assert(JSON.parse(result).fetch('status') == 'applied', 'edit_file did not commit')
      A.assert(File.read(File.join(workspace, 'visual-edit.txt')) == initial_edit.sub('EDIT_OLD_LINE', 'EDIT_NEW_LINE'), 'edit_file disk bytes differ')
      wait_diff(tui, 'visual-edit.txt', 1, 1)
      detail = A.assert_diff(tui.screen, 'visual-edit.txt', added: 1, removed: 1, rows: [
        {old: 1, new: 1, body: ' EDIT_CONTEXT_BEFORE'},
        {old: 2, new: nil, body: '-EDIT_OLD_LINE', color: :red},
        {old: nil, new: 2, body: '+EDIT_NEW_LINE', color: :green},
        {old: 3, new: 3, body: ' EDIT_CONTEXT_AFTER'}])
      detail.merge(snapshot(tui, directory, '05-edit-diff'))
    end

    batch = initial_batch.sub('BATCH_OLD_LINE', 'BATCH_NEW_LINE')
    preview = check(mode, 'file_changes preview does not write or display a committed diff') do
      baseline = A.tool(tui, provider, index, 'visual-read-batch', 'read_file', path: 'visual-batch.txt')
      index += 1
      A.assert(baseline.include?('BATCH_OLD_LINE'), 'batch baseline is absent')
      result = A.tool(tui, provider, index, 'visual-preview', 'file_changes', operation: 'preview', changes: [
        {operation: 'replace', path: 'visual-batch.txt', content: batch},
        {operation: 'create', path: 'visual-batch-new.txt', content: "BATCH_CREATED_LINE\n"}])
      index += 1
      value = JSON.parse(result)
      A.assert(value.fetch('status') == 'preview' && !value.key?('edit_display'), 'preview claimed a committed display')
      A.assert(File.read(File.join(workspace, 'visual-batch.txt')) == initial_batch, 'preview wrote replacement bytes')
      A.assert(!File.exist?(File.join(workspace, 'visual-batch-new.txt')), 'preview created a file')
      # Wait for a complete paint after the preview response/next request; no
      # frame cached from before the tool may establish the negative assertion.
      boundary = tui.raw.bytesize
      tui.wait('post-preview application frame', frame_started_after: boundary) { true }
      A.assert(!A.chat(tui.screen).include?('BATCH_NEW_LINE'), 'preview rendered as a committed diff')
      snapshot(tui, directory, '06-preview-only')
      value.fetch('preview_id')
    end

    check(mode, 'file_changes apply commits multi-file diffs matching disk contents') do
      result = A.tool(tui, provider, index, 'visual-apply', 'file_changes', operation: 'apply', preview_id: preview)
      index += 1
      value = JSON.parse(result)
      A.assert(value.fetch('status') == 'applied', 'file_changes apply did not commit')
      A.assert(value.fetch('files_touched').sort == %w[visual-batch-new.txt visual-batch.txt].sort, 'file_changes touched unexpected files')
      A.assert(File.read(File.join(workspace, 'visual-batch.txt')) == batch, 'batch replacement disk bytes differ')
      A.assert(File.read(File.join(workspace, 'visual-batch-new.txt')) == "BATCH_CREATED_LINE\n", 'batch create disk bytes differ')
      wait_diff(tui, 'visual-batch.txt', 1, 1)
      wait_diff(tui, 'visual-batch-new.txt', 1, 0)
      details = [A.assert_diff(tui.screen, 'visual-batch.txt', added: 1, removed: 1, rows: [
        {old: 1, new: 1, body: ' BATCH_CONTEXT_BEFORE'},
        {old: 2, new: nil, body: '-BATCH_OLD_LINE', color: :red},
        {old: nil, new: 2, body: '+BATCH_NEW_LINE', color: :green},
        {old: 3, new: 3, body: ' BATCH_CONTEXT_AFTER'}]),
        A.assert_diff(tui.screen, 'visual-batch-new.txt', added: 1, removed: 0, rows: [
          {old: nil, new: 1, body: '+BATCH_CREATED_LINE', color: :green}])]
      snapshot(tui, directory, '07-file-changes-applied').merge(files: details)
    end

    check(mode, 'all historical commands stay collapsed with final Provider held open') do
      collapsed_commands(tui.screen, gates)
      A.assert(provider.requests.length == index + 1, 'unexpected automatic verification/extra Provider request')
      A.assert(tui.clipboard_writes.empty?, 'fixture unexpectedly attempted a clipboard write')
      provider.assert_healthy!
      snapshot(tui, directory, '08-final-provider-held').merge(provider_requests: provider.requests.length,
        pending_response_index: index, clipboard_writes: tui.clipboard_writes.length)
    end
  rescue StandardError => error
    @report[:errors] << "#{mode}: #{error.class}: #{error.message}"
    warn "#{mode}: FAIL: #{error.message}"
  ensure
    actions = [
      ['capture last frame', -> { snapshot(tui, directory, 'last-frame') if tui&.screen }],
      ['save private Provider request evidence', -> { File.write(File.join(directory, 'provider-requests.json'), JSON.pretty_generate(provider.evidence)) if provider }],
      ['close owned PTY/VT decoder', -> { tui&.close }],
      ['release owned command locks', -> { gates&.each(&:release) }],
      ['stop only private Runtime', -> { interaction_command(environment, @binary, '--config', config, 'daemon', 'stop') if started_runtime }],
      ['close owned loopback Provider', -> { provider&.close }]
    ]
    actions.each do |name, action|
      action.call
      @report[:cleanup] << {mode: mode, action: name, passed: true}
    rescue StandardError => error
      @report[:cleanup] << {mode: mode, action: name, passed: false, error: "#{error.class}: #{error.message}"}
      @report[:errors] << "#{mode} cleanup #{name}: #{error.class}: #{error.message}"
    end
  end

  def write_reports
    File.write(File.join(@output, 'tool-visuals.json'), JSON.pretty_generate(@report))
    rows = @report[:checks].map { |item| "| #{item[:mode]} | #{item[:name]} | #{item[:passed] ? 'PASS' : 'FAIL'} |" }
    failures = @report[:errors].map { |error| "- #{error.gsub(/\s+/, ' ')}" }.join("\n")
    File.write(File.join(@output, 'tool-visuals.md'), <<~REPORT)
      # TUI 工具视觉真实 PTY 验收

      版本：#{@report[:version] || '未取得'}。结果：#{@report[:passed] ? 'PASS' : 'FAIL'}；通过 #{@report[:checks_passed]} 项检查。

      | 布局 | 检查 | 结果 |
      |---|---|---|
      #{rows.join("\n")}

      Binary：#{@binary}

      SHA256 前：#{@report[:binary_sha256_before]}

      SHA256 后：#{@report[:binary_sha256_after] || '未取得'}

      取证：full/compact 目录保留各状态的原始 ANSI、屏幕 JSON（含逐格颜色）、私有 Provider 请求和工具回执。

      隔离：#{@report[:isolation]}

      边界：#{@report[:boundary]}

      写工具后不发送最终 finish，不把自动验证或整轮完成作为本脚本通过条件。历史结束行来自写操作前实际完成的纯文本 Runtime 轮次。

      私有夹具：#{@report[:fixture_retained] ? "失败保留于 #{@root}" : '成功后已清理'}。Runtime 清理详情见 JSON 的 cleanup。

      #{failures.empty? ? '无失败项。' : failures}
    REPORT
  end
end

if $PROGRAM_NAME == __FILE__
  unless ARGV.length == 2
    warn 'Usage: ruby scripts/verify_tui_tool_visuals.rb BINARY OUTPUT_DIRECTORY'
    exit 64
  end
  binary = File.realpath(ARGV.fetch(0))
  raise 'BINARY must be an executable file' unless File.file?(binary) && File.executable?(binary)
  output = File.expand_path(ARGV.fetch(1))
  raise 'OUTPUT_DIRECTORY must be new or empty' if File.exist?(output) && (!File.directory?(output) || !Dir.empty?(output))
  FileUtils.mkdir_p(output, mode: 0o700)
  exit(ToolVisualRun.new(binary, output).run ? 0 : 1)
end
