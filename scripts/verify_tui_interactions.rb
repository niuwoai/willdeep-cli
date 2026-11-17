#!/usr/bin/env ruby
# frozen_string_literal: true

# Real Runtime/TUI regression with synthetic history, controlled loopback SSE,
# private HOME and no writes to the host clipboard.
# ruby scripts/verify_tui_interactions.rb BINARY OUTPUT_DIRECTORY
require 'digest'
require 'securerandom'
require 'tmpdir'
require_relative 'test/tui_interaction_fixture'

binary = File.realpath(ARGV.fetch(0))
output = File.expand_path(ARGV.fetch(1))
FileUtils.mkdir_p(output)
root = Dir.mktmpdir('wd-tui-interactions-')
checks = []
report = {passed: false, checks: checks, real_model_requests: false,
  binary_sha256: Digest::SHA256.file(binary).hexdigest,
  scope: 'full/compact real PTY: progress, accepted queue, reading anchor and draft preservation',
  isolation: 'Private HOME/WILLDEEP_HOME/workspace; loopback synthetic SSE; no inherited credentials.',
  clipboard: {transport: 'OSC 52 forced by private-process SSH_CONNECTION; captured PTY only, no host clipboard bridge', copies: []},
  resize_frames: []}

begin
  %w[compact full].each do |mode|
    directory = File.join(output, mode)
    user = File.join(root, mode, 'user')
    home = File.join(user, '.willdeep')
    workspace = File.join(root, mode, 'workspace')
    FileUtils.mkdir_p([directory, File.join(home, 'sessions'), workspace])
    environment = {'PATH' => ENV.fetch('PATH'), 'HOME' => user, 'USERPROFILE' => user,
      'WILLDEEP_HOME' => home, 'TERM' => 'xterm-256color', 'COLORTERM' => 'truecolor',
      'SSH_CONNECTION' => '127.0.0.1 65535 127.0.0.1 22',
      'WILLDEEP_LOCAL_DISCOVERY' => '0', 'WILLDEEP_NO_UPDATE_CHECK' => '1',
      'WILLDEEP_TELEMETRY_DISABLED' => '1', 'DO_NOT_TRACK' => '1', 'LANG' => 'en_US.UTF-8',
      'NO_PROXY' => '127.0.0.1,localhost', 'no_proxy' => '127.0.0.1,localhost',
      'XDG_CONFIG_HOME' => File.join(user, '.config'), 'XDG_CACHE_HOME' => File.join(user, '.cache'),
      'XDG_DATA_HOME' => File.join(user, '.local/share')}
    report[:version] ||= interaction_command(environment, binary, '--version').strip
    session_id = SecureRandom.uuid
    history = Array.new(36) do |index|
      text = Array.new(5) { |line| format('HIST%03d.%d 中文 original history line with stable coordinates.', index, line) }.join("\n")
      {role: index.even? ? 'user' : 'assistant', content: text}
    end
    session_path = File.join(home, 'sessions', "#{session_id}.json")
    File.write(session_path, JSON.generate({version: 1, id: session_id,
      title: 'Interaction fixture', workspace: workspace, profile: 'fixture',
      created_at: 1, updated_at: 1, messages: history, runtime_event_cursor: 0}))
    provider = InteractionProvider.new
    config = File.join(home, 'config.toml')
    File.write(config, ['version = 1', "default_provider = 'fixture'", '[agent]',
      "approval = 'smart'", 'max_turns = 3', 'auto_title = false', 'input_suggestions = false',
      'small_model_routing = false', 'auto_dispatch_read_only = false', '[ui]',
      "mode = '#{mode}'", 'check_updates = false', 'copy_on_select = false', '[providers.fixture]',
      "provider = 'openai-compatible'", "api = 'chat-completions'",
      "api_base = 'http://127.0.0.1:#{provider.port}/v1'", "api_key = 'fixture-only'",
      "model = 'fixture'", 'context_window = 2000000', ''].join("\n"), perm: 0o600)
    tui = nil
    check = lambda do |name, &assertion|
      assertion.call
      checks << {mode: mode, name: name, passed: true}
      puts "#{mode}: #{name}: PASS"
    rescue StandardError => error
      checks << {mode: mode, name: name, passed: false, error: error.message}
      raise
    end
    text = ->(screen) { screen.fetch('lines').join("\n") }
    new_content_pattern = /\b(?:\d+\s+)?new (?:content|updates|messages)\b(?:\s*[:(]?\s*\d+\)?)?/i
    new_content = lambda do |screen|
      screen.fetch('lines').filter_map do |line|
        next unless line.match?(new_content_pattern)
        # Check the complete row, including its blank tail. Extracting only the
        # phrase used to allow old history characters to survive underneath it.
        hint = line.strip.sub(/\A[└╰▔]/, '').sub(/[┘╯▔]\z/, '').strip
        raise "reading indicator contains stale cells: #{line.inspect}" unless hint == 'New content · End to latest'
        hint.downcase
      end
    end
    styles = ->(cells) { cells&.map { |cell| cell.values_at('bg', 'fg', 'inverse') } }
    begin
      tui = InteractionPty.new(binary, environment, config, workspace, session_id)
      tui.wait('history ready') { |screen| text.call(screen).include?('HIST035') }
      tui.paste('FIRST_INTERACTION_REQUEST')
      tui.pump(0.15)
      tui.write("\r")
      tui.wait('first provider request') { |_screen| provider.requests.length == 1 }
      check.call('model waiting is visible before output') do
        tui.wait('model waiting state') { |screen| text.call(screen).match?(/\bWaiting for model\b/) }
        tui.snapshot(directory, '01-waiting')
      end
      provider.delta(0, "Initial streamed answer.\n")
      tui.wait('initial stream') { |screen| text.call(screen).include?('Initial streamed answer.') }
      tui.wheel(up: true, count: 12)
      tui.wait('scrollback history') { |screen| text.call(screen).include?('HIST') && !text.call(screen).include?('Initial streamed answer.') }
      tui.pump(0.15)
      anchor = tui.screen.fetch('lines').each_with_index.find { |line, _y| line.match?(/HIST\d{3}\.\d/) }
      raise 'no visible history anchor' unless anchor
      marker = anchor.first[/HIST\d{3}\.\d/]
      anchor_y = anchor.last
      feedback_before = new_content.call(tui.screen)
      frame_before_increment = tui.raw.bytesize
      provider.delta(0, Array.new(12) { |index| "BACKGROUND#{index} streamed update.\n" }.join)
      check.call('streaming updates new-content feedback and preserves the visible history anchor') do
        tui.wait('changed new-content feedback', frame_started_after: frame_before_increment) do |screen|
          feedback = new_content.call(screen)
          !feedback.empty? && feedback != feedback_before
        end
        raise 'streaming moved the history anchor' unless tui.screen.fetch('lines')[anchor_y].include?(marker)
        tui.snapshot(directory, '02-scrollback')
      end
      selection_source = "#{marker} 中文 original history line with stable coordinates."
      source_cells = lambda do |screen|
        line = screen.fetch('lines').each_with_index.find { |value, _y| value.include?(selection_source) }
        next nil unless line
        cells = screen.fetch('cells').select { |cell| cell['y'] == line.last && cell['width'].positive? }
        cells.each_cons(selection_source.length).find { |range| range.map { |cell| cell['text'] }.join == selection_source }
      end
      original_cells = source_cells.call(tui.screen)
      raise 'complete history source is not visible' unless original_cells
      first, last = original_cells.first, original_cells.last
      original_styles = styles.call(original_cells)
      selected_styles = nil
      # Exercise the vulnerable gap between mouse-down and the first drag.
      tui.mouse_down(first)
      provider.delta(0, "BACKGROUND_DURING_MOUSE_DOWN streamed update.\n")
      tui.pump(0.3)
      tui.mouse_move(last)
      check.call('the full original selection survives incoming output') do
        tui.wait('complete selected history range') do |screen|
          actual = styles.call(source_cells.call(screen))
          actual && actual.zip(original_styles).all? { |selected, original| selected != original }
        end
        selected_styles = styles.call(source_cells.call(tui.screen))
        provider.delta(0, "BACKGROUND_AFTER_SELECTION new update.\n")
        tui.pump(0.5)
        before_mouse_up = tui.raw.bytesize
        tui.mouse_up(last)
        tui.wait('complete selection after incoming output', frame_started_after: before_mouse_up) do |screen|
          styles.call(source_cells.call(screen)) == selected_styles
        end
        raise 'selected history moved' unless tui.screen.fetch('lines')[anchor_y].include?(marker)
        tui.snapshot(directory, '03-selection')
      end
      check.call('the full original selection survives resize and copies through isolated OSC 52') do
        report[:resize_frames] << tui.resize(84, 32).merge(mode: mode)
        tui.wait('complete selected source after resize') do |screen|
          styles.call(source_cells.call(screen)) == selected_styles
        end
        tui.snapshot(directory, '03-selection-resized')
        report[:resize_frames] << tui.resize(100, 32).merge(mode: mode)
        tui.wait('complete selected source after restoring width') do |screen|
          styles.call(source_cells.call(screen)) == selected_styles
        end
        raise 'unexpected automatic clipboard copy' unless tui.clipboard_writes.empty?
        # Copy only after both resize checks: the copy shortcut may clear the
        # selection. The real terminal never receives this private PTY's OSC 52.
        tui.write("\x03")
        tui.wait('captured OSC 52 selection copy') { |_screen| !tui.clipboard_writes.empty? }
        tui.clipboard_writes.each do |copy|
          matched = copy.fetch(:text) == selection_source
          report[:clipboard][:copies] << copy.merge(mode: mode, expected_text: selection_source, matched: matched)
          raise 'OSC 52 copied text differs from the complete original selection' unless matched
        end
        tui.snapshot(directory, '03-selection-copy')
      end
      tui.write("\e")
      tui.pump(0.1)
      tui.paste('FOLLOWUP_INTERACTION_REQUEST')
      tui.wait('follow-up draft visible') { |screen| text.call(screen).include?('FOLLOWUP_INTERACTION_REQUEST') }
      draft_row = tui.screen.fetch('lines').index { |line| line.include?('FOLLOWUP_INTERACTION_REQUEST') }
      # Selection leaves Chat focused. Click the visible composer before Enter,
      # just as a user does when returning from reading to submitting a draft.
      tui.click(4, draft_row)
      tui.pump(0.15)
      tui.write("\r")
      check.call('busy follow-up is accepted without starting another provider request') do
        tui.wait('accepted queue feedback') { |screen| text.call(screen).match?(/queued|waiting in queue|waiting to send/i) }
        raise 'follow-up started before the first response finished' unless provider.requests.length == 1
        tui.snapshot(directory, '04-queued')
      end
      tui.write("\x11")
      check.call('queue exposes the actual priority-send effect') do
        tui.wait('queue action semantics') { |screen| text.call(screen).match?(/stop current|interrupt current|stop.*send|priorit/i) }
        tui.snapshot(directory, '05-queue-actions')
      end
      tui.write("\e")
      tui.pump(0.15)
      tui.paste('UNSENT_DRAFT_草稿😀')
      tui.pump(0.15)
      provider.finish(0)
      check.call('accepted follow-up starts exactly once and preserves the composer draft') do
        tui.wait('second request starts') { |_screen| provider.requests.length == 2 }
        raise 'wrong follow-up request' unless provider.requests[1].fetch(:messages).any? { |message| message['content'].to_s.include?('FOLLOWUP_INTERACTION_REQUEST') }
        tui.wait('draft remains') { |screen| text.call(screen).include?('UNSENT_DRAFT_草稿😀') }
        raise 'unexpected extra request' unless provider.requests.length == 2
        tui.snapshot(directory, '06-next-turn')
      end
      provider.delta(1, "SECOND_INTERACTION_REPLY\n")
      provider.finish(1)
      tui.wait('second response persisted before resolving the click target') do |_screen|
        saved = JSON.parse(File.read(session_path, encoding: 'UTF-8'))
        saved.fetch('messages').any? { |message| message['role'] == 'assistant' && message['content'].to_s.include?('SECOND_INTERACTION_REPLY') }
      end
      # Completion changes the full layout's activity height. Resolve the mouse
      # target from a fresh completed frame, not a buffered running-turn frame.
      completion_boundary = tui.raw.bytesize
      tui.wait('completed application frame before return-to-latest', frame_started_after: completion_boundary) do |screen|
        screen.fetch('lines').last.start_with?('✓') && !new_content.call(screen).empty?
      end
      tui.snapshot(directory, '06-completed-before-click')
      check.call('return-to-latest affordance reveals buffered output without sending the draft') do
        # The reading indicator is a direct mouse target in both layouts.
        indicator = tui.screen.fetch('lines').each_with_index.find { |line, _y| line.match?(new_content_pattern) }
        raise 'return-to-latest indicator missing' unless indicator
        tui.click(4, indicator.last)
        tui.wait('latest output') { |screen| text.call(screen).include?('SECOND_INTERACTION_REPLY') && text.call(screen).include?('UNSENT_DRAFT_草稿😀') }
        raise 'return-to-latest submitted the draft' unless provider.requests.length == 2
        tui.snapshot(directory, '07-latest')
      end
      check.call('completed synthetic responses were persisted by Runtime') do
        tui.wait('saved response') do |_screen|
          saved = JSON.parse(File.read(session_path, encoding: 'UTF-8'))
          saved.fetch('messages').any? { |message| message['role'] == 'assistant' && message['content'].to_s.include?('SECOND_INTERACTION_REPLY') }
        end
      end
    ensure
      tui.snapshot(directory, 'last-frame') if tui&.screen
      tui&.close
      provider.close
      File.binwrite(File.join(directory, 'terminal.ansi'), tui.raw) if tui
      interaction_command(environment, binary, '--config', config, 'daemon', 'stop')
    end
  end
  raise 'binary changed during terminal verification' unless Digest::SHA256.file(binary).hexdigest == report[:binary_sha256]
  report[:passed] = true
rescue StandardError => error
  report[:error] = "#{error.class}: #{error.message}"
  raise
ensure
  report[:checks_passed] = checks.count { |check| check[:passed] }
  File.write(File.join(output, 'integration.json'), JSON.pretty_generate(report))
  copies = report[:clipboard][:copies]
  File.write(File.join(output, 'integration.md'), "# TUI 交互真实终端验收\n\n#{report[:version]}\n\n" +
    checks.map { |check| "- #{check[:mode]} / #{check[:name]}：#{check[:passed] ? 'PASS' : 'FAIL'}" }.join("\n") +
    "\n\n结果：#{report[:passed] ? 'PASS' : 'FAIL'}\n#{report[:error]}\n\n" +
    "私有 HOME、工作区、假 Provider 与独立 PTY；模型请求仅发往本机合成服务。\n" +
    "剪贴板隔离：私有进程的 SSH_CONNECTION 强制 OSC 52，输出仅由夹具捕获，不转发到宿主终端或剪贴板。\n" +
    "动态证据：捕获 #{copies.length} 次复制，其中 #{copies.count { |copy| copy[:matched] }} 次与完整原文精确一致。" +
    "各次复制的预期原文、实际文本、字节数及缩放后的应用帧边界见 integration.json；未捕获不算通过。\n")
end
