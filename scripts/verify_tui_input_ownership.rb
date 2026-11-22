#!/usr/bin/env ruby
# frozen_string_literal: true

# Real Runtime and private PTY; never uses a real model, user daemon or clipboard.
require 'tmpdir'
require_relative 'test/support/tui_tool_visuals_support'

class InputOwnershipRun
  A = ToolVisualAcceptance
  CAPTURE_ON = "\e[?1006h".b
  CAPTURE_OFF = "\e[?1006l".b

  def initialize(binary, output)
    @binary, @output = binary, output
    @root = Dir.mktmpdir('wd-tui-input-')
    @report = {passed: false, checks: [], errors: [], cleanup: [], binary: binary,
      binary_sha256_before: Digest::SHA256.file(binary).hexdigest, fixture_root: @root,
      isolation: 'Private HOME/WILLDEEP_HOME/XDG/temp/workspace; allowlisted environment; synthetic loopback Provider; owned PTY/decoder; captured OSC52.',
      boundary: 'Validates actual application input/output in full/compact on macOS. Does not validate host terminal scrollback policy, real models or other operating systems.'}
  end

  def run
    begin
      %w[full compact].each { |mode| run_mode(mode) }
    rescue StandardError, Interrupt => error
      @report[:errors] << "#{error.class}: #{error.message}"
    ensure
      @report[:binary_sha256_after] = Digest::SHA256.file(@binary).hexdigest
      unchanged = @report[:binary_sha256_before] == @report[:binary_sha256_after]
      @report[:checks] << {mode: 'binary', name: 'binary SHA256 unchanged', passed: unchanged}
      @report[:passed] = unchanged && @report[:errors].empty? && @report[:checks].all? { |c| c[:passed] } &&
        @report[:cleanup].all? { |c| c[:passed] }
      File.write(File.join(@output, 'input-ownership.json'), JSON.pretty_generate(@report))
      rows = @report[:checks].map { |c| "| #{c[:mode]} | #{c[:name]} | #{c[:passed] ? 'PASS' : 'FAIL'} |" }
      File.write(File.join(@output, 'input-ownership.md'), <<~REPORT)
        # TUI 输入归属真实 PTY 验收

        版本：#{@report[:version]}。结果：#{@report[:passed] ? 'PASS' : 'FAIL'}。

        | 布局 | 检查 | 结果 |
        |---|---|---|
        #{rows.join("\n")}

        隔离：#{@report[:isolation]}

        边界：#{@report[:boundary]}

        Binary SHA256：#{@report[:binary_sha256_before]}；验收后：#{@report[:binary_sha256_after]}。

        私有夹具保留于 #{@root}。每个布局保留终端帧、ANSI 以及合成 Provider 请求和真实工具回执。

        #{@report[:errors].empty? ? '无失败项。' : @report[:errors].join("\n")}
      REPORT
      puts "TUI input ownership: #{@report[:passed] ? 'PASS' : 'FAIL'}; #{@report[:checks].count { |c| c[:passed] }} checks passed"
      warn @report[:errors].join("\n") unless @report[:errors].empty?
    end
    @report[:passed]
  end

  private

  def check(mode, name)
    evidence = yield
    @report[:checks] << {mode: mode, name: name, passed: true, evidence: evidence}
    puts "#{mode}: #{name}: PASS"
  rescue StandardError => error
    @report[:checks] << {mode: mode, name: name, passed: false, error: "#{error.class}: #{error.message}"}
    raise
  end

  def text(tui)
    tui.screen.fetch('lines').join("\n")
  end

  def cell(tui, label)
    # Use decoded cells rather than String offsets: wide glyphs consume two cells.
    tui.screen.fetch('lines').each_with_index do |line, y|
      next unless line.include?(label)
      cells = tui.screen.fetch('cells').select { |c| c.fetch('y') == y }
      cells.each do |candidate|
        tail = cells.select { |c| c.fetch('x') >= candidate.fetch('x') }.map { |c| c.fetch('text') }.join
        return [candidate.fetch('x'), y] if tail.start_with?(label)
      end
    end
    raise "visible control not found: #{label}"
  end

  def click(tui, label)
    tui.click(*cell(tui, label))
  end

  def gate(tui, provider, index, id, question, options, multi: false)
    names = provider.requests.fetch(index).fetch(:advertised_tools)
    A.assert(names.include?('ask_user'), 'Runtime did not advertise ask_user')
    provider.tool(index, id: id, name: 'ask_user', arguments: {question: question, options: options, multi_select: multi})
    tui.wait("question #{id}") do |screen|
      visible = screen.fetch('lines').join("\n")
      visible.include?('Question from Agent') && visible.include?(question)
    end
  end

  def receipt(tui, provider, expected, id)
    request = A.request(tui, provider, expected)
    matches = request.fetch(:messages).select { |m| m['role'] == 'tool' && m['tool_call_id'] == id }
    A.assert(matches.length == 1, "missing/duplicate tool receipt #{id}")
    matches.first.fetch('content')
  end

  def snapshot(tui, directory, name)
    tui.snapshot(directory, name)
    {screen: "#{name}.json", ansi: "#{name}.ansi", frame: tui.frame_serial}
  end

  def run_mode(mode)
    directory = File.join(@output, mode)
    FileUtils.mkdir_p(directory, mode: 0o700)
    environment = A.environment(File.join(@root, mode))
    workspace = File.join(@root, mode, 'workspace')
    FileUtils.mkdir_p(workspace, mode: 0o700)
    provider = ToolVisualProvider.new
    config, session = A.configure(environment, workspace, mode, provider)
    session_path = File.join(environment.fetch('WILLDEEP_HOME'), 'sessions', "#{session}.json")
    record = JSON.parse(File.read(session_path))
    record['messages'] = (1..40).map { |i| {role: 'assistant', content: "HISTORY_#{format('%02d', i)}\nprivate history line #{i}\nend of history #{i}"} }
    record['messages'] << {role: 'assistant', content: 'TOOL_VISUALS_READY'}
    File.write(session_path, JSON.generate(record))
    @report[:version] ||= interaction_command(environment, @binary, '--version').strip
    started = true
    interaction_command(environment, @binary, '--config', config, 'daemon', 'start')
    A.api(environment, @binary, config, 'workspace.register', root: workspace,
      name: 'Private input acceptance', access: 'full_access')
    A.api(environment, @binary, config, 'workspace.set_approval_mode', root: workspace,
      approval_mode: 'full_access', confirm_full_access: true)
    tui = ToolVisualPty.new(@binary, environment, config, workspace, session, columns: 132, rows: 40)
    tui.wait('private session') { |screen| A.chat(screen).include?('TOOL_VISUALS_READY') }
    check(mode, 'chat wheel browses history and input wheel leaves it in place') do
      history_pattern = /HISTORY_\d+|private history line \d+|end of history \d+/
      before = text(tui).scan(history_pattern)
      tui.wheel(up: true, count: 4, x: 5, y: 10)
      tui.wait('chat history moved') { |screen| screen.fetch('lines').join("\n").scan(history_pattern) != before && !A.chat(screen).include?('TOOL_VISUALS_READY') }
      history = text(tui).scan(history_pattern)
      tui.wheel(up: false, count: 3, x: 5, y: tui.rows - 2)
      4.times { tui.pump(0.1) }
      A.assert(text(tui).scan(history_pattern) == history, 'wheel in input changed the chat viewport')
      snapshot(tui, directory, '01-history')
    end
    # Clicking the composer also ensures ordinary text is entered in the input.
    tui.click(4, tui.rows - 3)
    A.submit(tui, provider, 'PRIVATE_INPUT_CHECK', 1)
    tui.paste('DRAFT_KEEP')
    tui.wait('draft') { text(tui).include?('DRAFT_KEEP') }
    check(mode, 'native selection freezes frames while background output continues') do
      tui.write("\x13")
      tui.wait('native selection frame') { text(tui).downcase.include?('native terminal selection') }
      3.times { tui.pump(0.1) }
      A.assert(tui.raw.rindex(CAPTURE_OFF).to_i > tui.raw.rindex(CAPTURE_ON).to_i, 'mouse capture was not disabled')
      frame = tui.frame_serial
      provider.delta(0, 'BACKGROUND_DURING_NATIVE')
      7.times { tui.pump(0.15) }
      A.assert(tui.frame_serial == frame, 'native selection emitted a new application frame')
      snapshot(tui, directory, '02-native-frozen').merge(frame_before: frame, frame_after: tui.frame_serial)
    end
    check(mode, 'asynchronous ask restores capture before direct mouse interaction') do
      gate(tui, provider, 0, 'ask-single', 'PRIVATE_SINGLE_QUESTION', %w[CHOICE_ALPHA CHOICE_BETA])
      A.assert(tui.raw.rindex(CAPTURE_ON).to_i > tui.raw.rindex(CAPTURE_OFF).to_i, 'question painted without restoring capture')
      snapshot(tui, directory, '03-question-restored')
      off_count = tui.raw.scan(CAPTURE_OFF).length
      tui.write("\x13")
      tui.write('qy')
      tui.wait('question text input') { text(tui).include?('qy') }
      A.assert(tui.raw.scan(CAPTURE_OFF).length == off_count, 'Ctrl+S disabled capture inside question')
      tui.write("\x7f\x7f")
      tui.wait('question edits cleared') { !text(tui).include?('qy') }
      click(tui, 'CHOICE_BETA')
      value = receipt(tui, provider, 2, 'ask-single')
      A.assert(value.include?('CHOICE_BETA'), "wrong single-selection receipt: #{value}")
      tui.wait('draft preserved') { text(tui).include?('DRAFT_KEEP') }
      {receipt: value, capture_restored: true, draft_preserved: true}
    end
    check(mode, 'multi-select uses visible controls and ignores outside clicks') do
      gate(tui, provider, 1, 'ask-multi', 'PRIVATE_MULTI_QUESTION', %w[MULTI_ALPHA MULTI_BETA], multi: true)
      tui.click(0, 0)
      3.times { tui.pump(0.1) }
      A.assert(provider.requests.length == 2 && text(tui).include?('Question from Agent'), 'outside click resolved the question')
      click(tui, 'MULTI_ALPHA')
      tui.wait('first checkbox') { text(tui).include?('[x] MULTI_ALPHA') }
      click(tui, 'MULTI_BETA')
      tui.wait('second checkbox') { text(tui).include?('[x] MULTI_BETA') }
      snapshot(tui, directory, '04-multi-selected')
      click(tui, '[ Send ]')
      value = receipt(tui, provider, 3, 'ask-multi')
      A.assert(value.include?('MULTI_ALPHA') && value.include?('MULTI_BETA'), "wrong multi receipt: #{value}")
      {receipt: value}
    end
    check(mode, 'long question scrolls and resized buttons keep their real hit locations') do
      # Runtime deliberately limits ask_user to 12 options. Use a short window
      # so the real, normalized menu overflows rather than relying on extra items.
      tui.resize(132, 18)
      gate(tui, provider, 2, 'ask-long', 'PRIVATE_LONG_QUESTION', (1..12).map { |i| "LONG_OPTION_#{format('%02d', i)}" }, multi: true)
      first = text(tui)
      tui.wheel(up: false, count: 5, x: 42, y: 12)
      tui.wait('question body scrolled') { text(tui) != first && !text(tui).include?('LONG_OPTION_01') }
      snapshot(tui, directory, '05-question-scrolled')
      tui.resize(48, 14)
      tui.wait('narrow fixed controls') { text(tui).include?('Skip') && text(tui).include?('Send') }
      snapshot(tui, directory, '06-question-narrow')
      click(tui, 'Skip')
      value = receipt(tui, provider, 4, 'ask-long')
      A.assert(!value.include?('LONG_OPTION_'), "skip submitted an option: #{value}")
      {receipt: value, resized: [48, 14]}
    end
    tui.resize(132, 40)
    provider.delta(3, 'PRIVATE_COMPLETED')
    provider.finish(3)
    # Native selection intentionally preserves the reading anchor. Return to the
    # latest content explicitly instead of assuming completion moves the reader.
    tui.write("\e[F")
    tui.wait('completed turn') { text(tui).include?('PRIVATE_COMPLETED') }
    check(mode, 'resize ends native selection and resumes drawing') do
      tui.resize(132, 40)
      tui.write("\x13")
      tui.wait('native selection again') { text(tui).downcase.include?('native terminal') }
      tui.resize(100, 32)
      tui.wait('resize restores interaction') { !text(tui).downcase.include?('native terminal selection') }
      A.assert(tui.raw.rindex(CAPTURE_ON).to_i > tui.raw.rindex(CAPTURE_OFF).to_i, 'resize did not restore capture')
      A.assert(tui.clipboard_writes.empty?, 'unexpected clipboard write')
      snapshot(tui, directory, '07-resized-restored')
    end
    check(mode, 'native paste restores capture and previews the unsent draft') do
      tui.resize(132, 40)
      tui.write("\x13")
      tui.wait('native before paste') { text(tui).downcase.include?('native terminal selection') }
      tui.paste('_PASTE_VISIBLE')
      tui.wait('paste previews draft') { text(tui).include?('DRAFT_KEEP_PASTE_VISIBLE') && !text(tui).downcase.include?('native terminal selection') }
      A.assert(tui.raw.rindex(CAPTURE_ON).to_i > tui.raw.rindex(CAPTURE_OFF).to_i, 'paste left capture disabled')
      A.assert(provider.requests.length == 4, 'paste unexpectedly submitted the draft')
      snapshot(tui, directory, '08-native-paste')
    end
  ensure
    [
      ['last frame', -> { snapshot(tui, directory, 'last-frame') if tui&.screen }],
      ['Provider evidence', -> { File.write(File.join(directory, 'provider-requests.json'), JSON.pretty_generate(provider.evidence)) if provider }],
      ['owned PTY/decoder', -> { tui&.close }],
      ['private Runtime', -> { interaction_command(environment, @binary, '--config', config, 'daemon', 'stop') if started }],
      ['owned Provider', -> { provider&.close }]
    ].each do |name, action|
      action.call
      @report[:cleanup] << {mode: mode, action: name, passed: true}
    rescue StandardError => error
      @report[:cleanup] << {mode: mode, action: name, passed: false, error: "#{error.class}: #{error.message}"}
      @report[:errors] << "#{mode} cleanup #{name}: #{error.class}: #{error.message}"
    end
  end
end

if $PROGRAM_NAME == __FILE__
  abort 'Usage: ruby scripts/verify_tui_input_ownership.rb BINARY OUTPUT_DIRECTORY' unless ARGV.length == 2
  binary = File.realpath(ARGV.fetch(0))
  raise 'BINARY must be executable' unless File.file?(binary) && File.executable?(binary)
  output = File.expand_path(ARGV.fetch(1))
  raise 'OUTPUT_DIRECTORY must be new or empty' if File.exist?(output) && (!File.directory?(output) || !Dir.empty?(output))
  FileUtils.mkdir_p(output, mode: 0o700)
  exit(InputOwnershipRun.new(binary, output).run ? 0 : 1)
end
