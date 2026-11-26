#!/usr/bin/env ruby
# frozen_string_literal: true

# Real Runtime + PTY acceptance for turn-end projection; no real model, user
# HOME, clipboard or Git. Usage:
#   ruby scripts/verify_tui_turn_end_projection.rb BINARY OUTPUT_DIRECTORY
# OUTPUT_DIRECTORY must be new or empty. Failed fixtures/evidence are retained.
require 'tmpdir'
require_relative 'test/support/tui_tool_visuals_support'

class TurnEndProjectionRun
  A = ToolVisualAcceptance
  FILE = 'turn-end-projection.txt'
  LINES = %w[TURN_END_FIRST_LINE TURN_END_MIDDLE_LINE TURN_END_LAST_LINE].freeze

  def initialize(binary, output)
    @binary, @output = binary, output
    @root = Dir.mktmpdir('wd-tui-turn-end-')
    @report = {passed: false, checks: [], errors: [], cleanup: [],
      binary: binary, binary_sha256_before: Digest::SHA256.file(binary).hexdigest,
      fixture_root: @root, real_model_requests: false, layouts: %w[full compact],
      version: nil,
      scope: 'real Runtime/full+compact PTY; committed create_file diff while running, /version during a live reply, automatic fold after the final result, click-to-reopen',
      isolation: 'Private HOME/WILLDEEP_HOME/XDG/temp/workspace; allowlisted child environment; synthetic loopback Provider; OSC52 captured in owned PTY.',
      boundary: 'Synthetic loopback Provider only; no real model or user Runtime. Session-switch, failed-send and manual-expansion persistence are covered by Rust native regressions, not this PTY.'}
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
      puts "TUI turn-end projection: #{@report[:passed] ? 'PASS' : 'FAIL'}; #{@report[:checks_passed]} checks passed"
      puts "Report: #{File.join(@output, 'turn-end-projection.json')}"
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

  def wait_diff(tui)
    tui.wait("committed diff for #{FILE}", seconds: A::WAIT_SECONDS) do |screen|
      A.chat(screen).match?(/#{Regexp.escape(FILE)}\s+\+#{LINES.length}\s+[−-]0(?!\d)/) &&
        A.chat(screen).include?("+#{LINES.last}")
    end
  end

  def diff_rows
    LINES.each_with_index.map do |line, index|
      {old: nil, new: index + 1, body: "+#{line}", color: :green}
    end
  end

  def run_mode(mode)
    directory = File.join(@output, mode)
    FileUtils.mkdir_p(directory, mode: 0o700)
    private_root = File.join(@root, mode)
    environment = A.environment(private_root)
    workspace = File.join(private_root, 'workspace')
    FileUtils.mkdir_p(workspace, mode: 0o700)
    provider, tui, config, started_runtime = nil, nil, nil, false
    provider = ToolVisualProvider.new
    config, session = A.configure(environment, workspace, mode, provider)
    @report[:version] = interaction_command(environment, @binary, '--version').strip
    started_runtime = true
    interaction_command(environment, @binary, '--config', config, 'daemon', 'start')
    A.api(environment, @binary, config, 'workspace.register', root: workspace,
      name: 'Private TUI turn-end acceptance', access: 'full_access')
    A.api(environment, @binary, config, 'workspace.set_approval_mode', root: workspace,
      approval_mode: 'full_access', confirm_full_access: true)
    tui = ToolVisualPty.new(@binary, environment, config, workspace, session)
    tui.wait('private resumed session') { |screen| A.chat(screen).include?('TOOL_VISUALS_READY') }

    A.submit(tui, provider, 'TURN_END_PROJECTION_REQUEST', 1)
    receipt = A.tool(tui, provider, 0, 'turn-end-create', 'create_file', path: FILE, content: LINES.join("\n") + "\n")
    A.assert(JSON.parse(receipt).fetch('status') == 'created', 'create_file did not return a committed receipt')
    A.assert(File.read(File.join(workspace, FILE)) == LINES.join("\n") + "\n", 'create_file disk bytes differ')

    check(mode, 'committed edit diff is visible while the turn is still running') do
      wait_diff(tui)
      A.assert_diff(tui.screen, FILE, added: LINES.length, removed: 0, rows: diff_rows)
      A.assert(provider.requests.length == 2, "turn started an unexpected extra Provider request: #{provider.requests.length}")
      snapshot(tui, directory, '01-running-diff')
    end

    check(mode, '/version shows both versions after one Enter during a live reply') do
      tui.paste('/tools')
      tui.write("\r")
      tui.wait('first local command feedback') { |screen| A.chat(screen).match?(/tool-call rows (folded|expanded)/i) }
      tui.paste('/tools')
      tui.write("\r")
      tui.wait('adjacent System notices fold') { |screen| A.chat(screen).include?('2 system messages') }
      provider.delta(1, "TURN_END_STREAMING_REPLY\n")
      tui.wait('model reply is streaming') { |screen| A.chat(screen).include?('TURN_END_STREAMING_REPLY') }
      tui.paste('/version')
      tui.wait('exact /version suggestion is visible') do |screen|
        screen.fetch('lines').any? { |line| line.include?('/version') && line.include?('Runtime') }
      end
      tui.write("\r")
      version = @report.fetch(:version).split.last
      tui.wait('CLI and Runtime versions in chat', seconds: 5) do |screen|
        A.chat(screen).include?("CLI #{version} · Runtime #{version}")
      end
      A.assert(provider.requests.length == 2, '/version triggered a Provider request')
      A.assert(!A.chat(tui.screen).include?('turn finished'), '/version appeared only after the turn finished')
      snapshot(tui, directory, '02-version-during-reply')
    end

    provider.delta(1, "TURN_END_FINAL_ANSWER\n")
    provider.finish(1)
    check(mode, 'the final result folds the intermediate diff and keeps the tool receipt') do
      tui.wait('turn finished without the diff', seconds: A::WAIT_SECONDS) do |screen|
        chat = A.chat(screen)
        chat.include?('TURN_END_FINAL_ANSWER') && chat.include?('turn finished') &&
          !chat.include?("+#{LINES.first}") && !chat.include?(FILE)
      end
      chat = A.chat(tui.screen)
      A.assert(!chat.include?("+#{LINES.last}"), 'committed diff survived the final result')
      A.assert(!A.chat_lines(tui.screen).any? { |line| line.include?(FILE) }, 'diff file summary survived the final result')
      A.assert(chat.include?('✓ create_file'), 'tool receipt row disappeared with the diff')
      A.assert(provider.requests.length == 2, 'final result triggered an unexpected Provider request')
      snapshot(tui, directory, '03-after-final-result')
    end

    check(mode, 'clicking the retained tool row reopens the committed diff') do
      row = A.chat_lines(tui.screen).index { |line| line.include?('✓ create_file') }
      A.assert(row, 'retained create_file row is outside the captured chat')
      tui.click(4, row)
      wait_diff(tui)
      A.assert_diff(tui.screen, FILE, added: LINES.length, removed: 0, rows: diff_rows)
      A.assert(provider.requests.length == 2, 'reopening the diff triggered a Provider request')
      snapshot(tui, directory, '04-reopened-diff')
    end
  rescue StandardError => error
    @report[:errors] << "#{mode}: #{error.class}: #{error.message}"
    warn "#{mode}: FAIL: #{error.message}"
  ensure
    actions = [
      ['capture last frame', -> { snapshot(tui, directory, 'last-frame') if tui&.screen }],
      ['save private Provider request evidence', -> { File.write(File.join(directory, 'provider-requests.json'), JSON.pretty_generate(provider.evidence)) if provider }],
      ['close owned PTY/VT decoder', -> { tui&.close }],
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
    File.write(File.join(@output, 'turn-end-projection.json'), JSON.pretty_generate(@report))
    rows = @report[:checks].map { |item| "| #{item[:mode]} | #{item[:name]} | #{item[:passed] ? 'PASS' : 'FAIL'} |" }
    failures = @report[:errors].map { |error| "- #{error.gsub(/\s+/, ' ')}" }.join("\n")
    File.write(File.join(@output, 'turn-end-projection.md'), <<~REPORT)
      # TUI 轮次收尾投影真实 PTY 验收

      版本：#{@report[:version] || '未取得'}。结果：#{@report[:passed] ? 'PASS' : 'FAIL'}；通过 #{@report[:checks_passed]} 项检查。

      | 布局 | 检查 | 结果 |
      |---|---|---|
      #{rows.join("\n")}

      Binary：#{@binary}

      SHA256 前：#{@report[:binary_sha256_before]}

      SHA256 后：#{@report[:binary_sha256_after] || '未取得'}

      取证：full/compact 目录保留运行中、版本回执、最终结果后和点击重开的原始 ANSI、屏幕 JSON（含逐格颜色）与私有 Provider 请求。

      隔离：#{@report[:isolation]}

      边界：#{@report[:boundary]}

      私有夹具：#{@report[:fixture_retained] ? "失败保留于 #{@root}" : '成功后已清理'}。Runtime 清理详情见 JSON 的 cleanup。

      #{failures.empty? ? '无失败项。' : failures}
    REPORT
  end
end

if $PROGRAM_NAME == __FILE__
  unless ARGV.length == 2
    warn 'Usage: ruby scripts/verify_tui_turn_end_projection.rb BINARY OUTPUT_DIRECTORY'
    exit 64
  end
  binary = File.realpath(ARGV.fetch(0))
  raise 'BINARY must be an executable file' unless File.file?(binary) && File.executable?(binary)
  output = File.expand_path(ARGV.fetch(1))
  raise 'OUTPUT_DIRECTORY must be new or empty' if File.exist?(output) && (!File.directory?(output) || !Dir.empty?(output))
  FileUtils.mkdir_p(output, mode: 0o700)
  exit(TurnEndProjectionRun.new(binary, output).run ? 0 : 1)
end
