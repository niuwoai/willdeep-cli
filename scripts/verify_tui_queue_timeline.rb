#!/usr/bin/env ruby
# frozen_string_literal: true

# Real Runtime/TUI regression: queued messages move after the preceding turn,
# and the queue acknowledgement disappears when its message starts.
# ruby scripts/verify_tui_queue_timeline.rb BINARY OUTPUT_DIRECTORY
require 'cgi'
require 'digest'
require 'securerandom'
require 'tmpdir'
require_relative 'test/tui_interaction_fixture'

binary = File.realpath(ARGV.fetch(0))
output = File.expand_path(ARGV.fetch(1))
FileUtils.mkdir_p(output)
root = Dir.mktmpdir('wd-tui-queue-timeline-')
report = {passed: false, checks: [], real_model_requests: false,
  binary_sha256: Digest::SHA256.file(binary).hexdigest,
  scope: 'full/compact real Runtime PTY; two queued turns, late preceding output, receipt cleanup and draft preservation',
  isolation: 'Private HOME/WILLDEEP_HOME/workspace, loopback synthetic provider, captured OSC 52 only.'}
text = ->(screen) { screen.fetch('lines').join("\n") }
# The full layout also previews queued text in Activity. Count only chat rows
# when asserting transcript identity/order, rather than counting that preview.
conversation = ->(screen) { screen.fetch('lines').take_while { |line| !line.start_with?('┌Activity', '┌Prompt') }.join("\n") }
queue_notice = /Saved to (?:the )?queue|not (?:yet )?delivered to (?:the )?(?:current )?model|Runtime accepted and queued the message|waiting to start/i
render_frame = lambda do |screen, path|
  cell_width, cell_height = 10, 20
  cells = screen.fetch('cells')
  width = (cells.map { |cell| cell.fetch('x') }.max + 1) * cell_width + 32
  height = (cells.map { |cell| cell.fetch('y') }.max + 1) * cell_height + 32
  color = ->(value, fallback) { value.is_a?(Integer) && value.positive? ? format('#%06x', value) : fallback }
  svg = [%(<svg xmlns="http://www.w3.org/2000/svg" width="#{width}" height="#{height}" viewBox="0 0 #{width} #{height}">),
    '<rect width="100%" height="100%" fill="#1b2232"/>']
  cells.each do |cell|
    next unless cell.fetch('bg').positive?
    svg << %(<rect x="#{16 + cell.fetch('x') * cell_width}" y="#{16 + cell.fetch('y') * cell_height}" width="#{cell_width}" height="#{cell_height}" fill="#{color.call(cell.fetch('bg'), '#1b2232')}"/>)
  end
  svg << %(<g font-family="Menlo,monospace" font-size="16">)
  cells.each do |cell|
    next if cell.fetch('width').zero? || cell.fetch('text').strip.empty?
    svg << %(<text x="#{16 + cell.fetch('x') * cell_width}" y="#{32 + cell.fetch('y') * cell_height}" fill="#{color.call(cell.fetch('fg'), '#c0c7d2')}">#{CGI.escapeHTML(cell.fetch('text'))}</text>)
  end
  svg << '</g></svg>'
  File.write(path, svg.join("\n"))
end

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
    session_path = File.join(home, 'sessions', "#{session_id}.json")
    File.write(session_path, JSON.generate({version: 1, id: session_id,
      title: 'Queue timeline fixture', workspace: workspace, profile: 'fixture',
      created_at: 1, updated_at: 1,
      messages: [{role: 'assistant', content: 'QUEUE_TIMELINE_READY'}], runtime_event_cursor: 0}))
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
      report[:checks] << {mode: mode, name: name, passed: true}
      puts "#{mode}: #{name}: PASS"
    rescue StandardError => error
      report[:checks] << {mode: mode, name: name, passed: false, error: error.message}
      raise
    end
    begin
      tui = InteractionPty.new(binary, environment, config, workspace, session_id, columns: 110, rows: 42)
      tui.wait('ready') { |screen| text.call(screen).include?('QUEUE_TIMELINE_READY') }
      tui.paste('FIRST_TIMELINE_REQUEST')
      tui.pump(0.1)
      tui.write("\r")
      tui.wait('first request') { |_screen| provider.requests.length == 1 }
      provider.delta(0, "FIRST_TIMELINE_HEAD\n")
      tui.wait('first output') { |screen| text.call(screen).include?('FIRST_TIMELINE_HEAD') }
      %w[QUEUED_TIMELINE_ONE QUEUED_TIMELINE_TWO].each do |message|
        tui.paste(message)
        tui.pump(0.1)
        tui.write("\r")
        tui.wait("queue #{message}") { |screen| text.call(screen).include?(message) && text.call(screen).match?(/queued|queue|waiting to send/i) }
      end
      check.call('queued inputs do not start early') do
        raise 'queued turn started before first completion' unless provider.requests.length == 1
        tui.snapshot(directory, '01-queued')
      end
      tui.paste('UNSENT_TIMELINE_DRAFT')
      tui.pump(0.1)
      provider.delta(0, "FIRST_TIMELINE_TAIL\n")
      tui.wait('late first output') { |screen| text.call(screen).include?('FIRST_TIMELINE_TAIL') }
      provider.finish(0)
      tui.wait('second request') { |_screen| provider.requests.length == 2 }
      provider.delta(1, "SECOND_TIMELINE_REPLY\n")
      tui.wait('second output') { |screen| text.call(screen).include?('SECOND_TIMELINE_REPLY') }
      check.call('first dequeued user follows the complete preceding reply') do
        screen = conversation.call(tui.screen)
        tail = screen.index('FIRST_TIMELINE_TAIL')
        user_row = screen.index('QUEUED_TIMELINE_ONE')
        reply = screen.index('SECOND_TIMELINE_REPLY')
        raise "wrong first dequeue order: #{screen}" unless tail && user_row && reply && tail < user_row && user_row < reply
        raise 'first user echoed more than once' unless screen.scan('QUEUED_TIMELINE_ONE').length == 1
        tui.snapshot(directory, '02-first-dequeued')
      end
      provider.finish(1)
      tui.wait('third request') { |_screen| provider.requests.length == 3 }
      provider.delta(2, "THIRD_TIMELINE_REPLY\n")
      tui.wait('third output') { |screen| text.call(screen).include?('THIRD_TIMELINE_REPLY') }
      # The Activity queue count uses the periodic Runtime snapshot. Capture the
      # proof after that snapshot catches up, while the third reply stays live.
      tui.wait('queue snapshot drained') { |screen| !text.call(screen).match?(/\bqueued\s+[1-9]\d*/i) }
      check.call('all dequeued messages and replies follow actual turn order') do
        screen = conversation.call(tui.screen)
        markers = %w[FIRST_TIMELINE_HEAD FIRST_TIMELINE_TAIL QUEUED_TIMELINE_ONE SECOND_TIMELINE_REPLY QUEUED_TIMELINE_TWO THIRD_TIMELINE_REPLY]
        positions = markers.map { |marker| screen.index(marker) }
        raise "wrong complete order: #{screen}" unless positions.none?(&:nil?) && positions.each_cons(2).all? { |left, right| left < right }
        %w[QUEUED_TIMELINE_ONE QUEUED_TIMELINE_TWO].each do |marker|
          raise "duplicate user row: #{marker}" unless screen.scan(marker).length == 1
        end
        tui.snapshot(directory, '03-all-dequeued')
        render_frame.call(tui.screen, File.join(directory, '03-all-dequeued.svg'))
      end
      check.call('queue acknowledgement is gone after delivery and draft remains') do
        screen = text.call(tui.screen)
        raise 'stale queue acknowledgement remains' if screen.match?(queue_notice)
        raise 'unsent draft was lost' unless screen.include?('UNSENT_TIMELINE_DRAFT')
        raise 'draft was submitted' unless provider.requests.length == 3
      end
      provider.finish(2)
      check.call('Runtime persists the same conversation order') do
        tui.wait('persisted final output') do |_screen|
          saved = JSON.parse(File.read(session_path, encoding: 'UTF-8'))
          saved.fetch('messages').any? { |message| message['content'].to_s.include?('THIRD_TIMELINE_REPLY') }
        end
        messages = JSON.parse(File.read(session_path, encoding: 'UTF-8')).fetch('messages')
        markers = %w[FIRST_TIMELINE_REQUEST FIRST_TIMELINE_TAIL QUEUED_TIMELINE_ONE SECOND_TIMELINE_REPLY QUEUED_TIMELINE_TWO THIRD_TIMELINE_REPLY]
        positions = markers.map { |marker| messages.index { |message| message['content'].to_s.include?(marker) } }
        raise 'persisted history order differs' unless positions.none?(&:nil?) && positions.each_cons(2).all? { |left, right| left < right }
      end
    ensure
      tui.snapshot(directory, 'last-frame') if tui&.screen
      tui&.close
      provider.close
      interaction_command(environment, binary, '--config', config, 'daemon', 'stop')
    end
  end
  raise 'binary changed during verification' unless Digest::SHA256.file(binary).hexdigest == report[:binary_sha256]
  report[:passed] = true
rescue StandardError => error
  report[:error] = "#{error.class}: #{error.message}"
  raise
ensure
  report[:checks_passed] = report[:checks].count { |check| check[:passed] }
  File.write(File.join(output, 'queue-timeline.json'), JSON.pretty_generate(report))
  rows = report[:checks].map { |check| "| #{check[:mode]} | #{check[:name]} | #{check[:passed] ? 'PASS' : 'FAIL'} |" }
  File.write(File.join(output, 'queue-timeline.md'), "# TUI 队列时间线验收\n\n#{report[:version]}\n\n通过：#{report[:passed]}；检查：#{report[:checks_passed]}。\n\n| 布局 | 检查 | 结果 |\n|---|---|---|\n#{rows.join("\n")}\n\n隔离：#{report[:isolation]}\n\n仅覆盖本机合成 Provider 和 Runtime TUI，不代表真实模型、跨平台或 /local 的端到端验证。\n\n#{report[:error]}\n")
  FileUtils.remove_entry(root) if File.directory?(root)
end
