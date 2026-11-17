#!/usr/bin/env ruby
# frozen_string_literal: true

# ruby scripts/verify_tui_status_symbols.rb BINARY OUTPUT_DIRECTORY
require 'cgi'
require 'digest'
require 'securerandom'
require 'tmpdir'
require_relative 'test/tui_interaction_fixture'

binary = File.realpath(ARGV.fetch(0))
output = File.expand_path(ARGV.fetch(1))
FileUtils.mkdir_p(output)
root = Dir.mktmpdir('wd-tui-status-symbols-')
report = {passed: false, checks: [], real_model_requests: false,
  binary_sha256: Digest::SHA256.file(binary).hexdigest,
  scope: 'full/compact Runtime TUI; status symbols, footer click/drag, resize, isolated OSC 52 copy and unsent draft',
  isolation: 'Private HOME/WILLDEEP_HOME/workspace; loopback synthetic SSE; no real credentials or clipboard.', cleanup: []}
text = ->(screen) { screen.fetch('lines').join("\n") }
chat = ->(screen) { screen.fetch('lines').take_while { |line| !line.start_with?('┌Activity', '┌Prompt') } }
save_frame = lambda do |tui, directory, name|
  screen = tui.screen
  # Keep terminal evidence once per case; render the same captured cells as SVG.
  File.write(File.join(directory, "#{name}.json"), JSON.pretty_generate(screen))
  cells = screen.fetch('cells')
  width, height = tui.columns * 10 + 32, tui.rows * 20 + 32
  color = ->(value, fallback) { value.is_a?(Integer) && value.positive? ? format('#%06x', value) : fallback }
  svg = [%(<svg xmlns="http://www.w3.org/2000/svg" width="#{width}" height="#{height}">),
    '<rect width="100%" height="100%" fill="#1b2232"/>']
  cells.each do |cell|
    next unless cell.fetch('bg').positive?
    svg << %(<rect x="#{16 + cell.fetch('x') * 10}" y="#{16 + cell.fetch('y') * 20}" width="#{10 * [cell.fetch('width'), 1].max}" height="20" fill="#{color.call(cell.fetch('bg'), '#1b2232')}"/>)
  end
  svg << '<g font-family="Menlo,monospace" font-size="16">'
  cells.each do |cell|
    next if cell.fetch('width').zero? || cell.fetch('text').strip.empty?
    svg << %(<text x="#{16 + cell.fetch('x') * 10}" y="#{32 + cell.fetch('y') * 20}" fill="#{color.call(cell.fetch('fg'), '#c0c7d2')}">#{CGI.escapeHTML(cell.fetch('text'))}</text>)
  end
  svg << '</g></svg>'
  File.write(File.join(directory, "#{name}.svg"), svg.join("\n"))
end

begin
  %w[compact full].product(%w[en zh-CN]).each do |mode, language|
    turn_label = language == 'en' ? 'your turn' : '轮到你'
    case_name = "#{mode}-#{language}"
    directory = File.join(output, case_name)
    user = File.join(root, case_name, 'user')
    home = File.join(user, '.willdeep')
    workspace = File.join(root, case_name, 'workspace')
    FileUtils.mkdir_p([directory, File.join(home, 'sessions'), workspace])
    environment = {'PATH' => ENV.fetch('PATH'), 'HOME' => user, 'USERPROFILE' => user,
      'WILLDEEP_HOME' => home, 'TERM' => 'xterm-256color', 'COLORTERM' => 'truecolor',
      'SSH_CONNECTION' => '127.0.0.1 65535 127.0.0.1 22', 'LANG' => 'en_US.UTF-8',
      'WILLDEEP_LOCAL_DISCOVERY' => '0', 'WILLDEEP_NO_UPDATE_CHECK' => '1',
      'WILLDEEP_TELEMETRY_DISABLED' => '1', 'DO_NOT_TRACK' => '1',
      'NO_PROXY' => '127.0.0.1,localhost', 'no_proxy' => '127.0.0.1,localhost',
      'XDG_CONFIG_HOME' => File.join(user, '.config'), 'XDG_CACHE_HOME' => File.join(user, '.cache'),
      'XDG_DATA_HOME' => File.join(user, '.local/share')}
    report[:version] ||= interaction_command(environment, binary, '--version').strip
    session_id = SecureRandom.uuid
    session_path = File.join(home, 'sessions', "#{session_id}.json")
    File.write(session_path, JSON.generate({version: 1, id: session_id,
      title: 'Status symbol fixture', workspace: workspace, profile: 'fixture', created_at: 1,
      updated_at: 1, messages: [{role: 'assistant', content: Array.new(48) { |index| "HISTORY_#{index} 中文 history fills the viewport." }.join("\n") + "\nSTATUS_SYMBOL_READY"}], runtime_event_cursor: 0}))
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
      report[:checks] << {mode: mode, language: language, name: name, passed: true}
      puts "#{mode}: #{name}: PASS"
    rescue StandardError => error
      report[:checks] << {mode: mode, language: language, name: name, passed: false, error: error.message}
      raise
    end
    begin
      tui = InteractionPty.new(binary, environment, config, workspace, session_id, columns: 132, rows: 36, language: language)
      tui.wait('ready') { |screen| text.call(screen).include?('STATUS_SYMBOL_READY') }
      tui.paste('STATUS_SYMBOL_FIRST')
      tui.pump(0.1)
      tui.write("\r")
      tui.wait('first request') { |_screen| provider.requests.length == 1 }
      provider.delta(0, "STATUS_SYMBOL_FIRST_REPLY\n")
      tui.wait('first reply') { |screen| text.call(screen).include?('STATUS_SYMBOL_FIRST_REPLY') }
      check.call('started message uses a symbol without the old status phrase') do
        tui.wait('started symbol') { |screen| chat.call(screen).any? { |line| line.include?('▶ STATUS_SYMBOL_FIRST') } }
        raise 'old textual status remains' if chat.call(tui.screen).join.match?(/\[(?:started|completed|accepted)/)
      end
      tui.paste('STATUS_SYMBOL_QUEUED')
      tui.pump(0.1)
      tui.write("\r")
      check.call('queued symbol does not claim early start or completion') do
        tui.wait('queued symbol') { |screen| chat.call(screen).any? { |line| line.include?('◇ STATUS_SYMBOL_QUEUED') } }
        raise 'queued message started early' unless provider.requests.length == 1
        save_frame.call(tui, directory, '01-queued')
      end
      tui.paste('UNSENT_STATUS_DRAFT')
      tui.pump(0.1)
      provider.finish(0)
      tui.wait('second request') { |_screen| provider.requests.length == 2 }
      provider.delta(1, "STATUS_SYMBOL_FINAL_REPLY\n")
      provider.finish(1)
      tui.wait('final reply and divider') { |screen| text.call(screen).include?('STATUS_SYMBOL_FINAL_REPLY') && chat.call(screen).any? { |line| line.include?('── ') && line.include?(turn_label) } }
      check.call('normal terminal status comes from the completed turn') do
        tui.wait('completed symbols') { |screen| chat.call(screen).count { |line| line.match?(/✓ STATUS_SYMBOL_(?:FIRST|QUEUED)/) } == 2 }
        raise 'routine completion still uses repeated status words' if text.call(tui.screen).match?(/本轮已完成|Turn completed|ターン完了/)
      end
      summary = chat.call(tui.screen).find { |line| line.include?('── ') && line.include?(turn_label) }.strip
      save_frame.call(tui, directory, '02-before-click')
      row_y = tui.screen.fetch('lines').index { |line| line.strip == summary }
      summary_cells = tui.screen.fetch('cells').select { |cell| cell['y'] == row_y && cell['x'].between?(1, tui.columns - 2) && cell['width'].positive? && !cell['text'].strip.empty? }
      first, last = summary_cells.first, summary_cells.last
      boundary = tui.raw.bytesize
      tui.mouse_down(first)
      check.call('mouse-down keeps the complete footer and puts reading feedback on another row') do
        tui.wait('reading feedback after mouse-down', frame_started_after: boundary) { |screen| text.call(screen).match?(/End.*(?:latest|最新)/i) }
        raise 'mouse-down replaced footer text' unless chat.call(tui.screen).any? { |line| line.strip == summary }
        raise 'reading feedback overlaps footer' if tui.screen.fetch('lines').any? { |line| line.include?(summary) && line.match?(/End.*(?:latest|最新)/i) }
      end
      tui.mouse_move(last)
      tui.mouse_up(last)
      tui.pump(0.2)
      check.call('drag-selection preserves footer text and draft') do
        raise 'selection changed footer text' unless chat.call(tui.screen).any? { |line| line.strip == summary }
        raise 'draft lost during selection' unless text.call(tui.screen).include?('UNSENT_STATUS_DRAFT')
        save_frame.call(tui, directory, '03-selected')
      end
      # A real resize and restoring width must not cause a mixed footer frame.
      tui.resize(90, 36)
      tui.resize(132, 36)
      check.call('resize preserves footer text and isolated selection copying') do
        tui.wait('restored footer') { |screen| chat.call(screen).any? { |line| line.strip == summary } }
        # Generated statistics are deliberately excluded from source copying.
        # Copy the real message body, including a drag across its receipt symbol.
        body = 'STATUS_SYMBOL_QUEUED'
        user_y = tui.screen.fetch('lines').index { |line| line.include?("✓ #{body}") }
        raise 'completed user message missing after resize' unless user_y
        user_cells = tui.screen.fetch('cells').select { |cell| cell['y'] == user_y && cell['width'].positive? }
        body_cells = user_cells.each_cons(body.length).find { |range| range.map { |cell| cell['text'] }.join == body }
        receipt = user_cells.find { |cell| cell['text'] == '✓' }
        raise 'user body or receipt cells missing' unless body_cells && receipt
        tui.drag(receipt, body_cells.last)
        tui.pump(0.1)
        tui.write("\x03")
        tui.wait('body copied through captured OSC 52') { |_screen| !tui.clipboard_writes.empty? }
        copied = tui.clipboard_writes.last.fetch(:text)
        raise "copy contains decoration or lost body: #{copied.inspect}" unless copied == body
        raise 'draft was submitted' unless provider.requests.length == 2
        (report[:body_copies] ||= []) << {mode: mode, language: language, text: copied}
        save_frame.call(tui, directory, '04-resized-copy')
      end
      tui.write("\e")
      tui.pump(0.1)
      indicator = tui.screen.fetch('lines').index { |line| line.match?(/End.*(?:latest|最新)/i) }
      raise 'reading indicator missing' unless indicator
      tui.click(4, indicator)
      check.call('return-to-latest preserves the footer and unsent draft') do
        tui.wait('reading feedback cleared') { |screen| !text.call(screen).match?(/End.*(?:latest|最新)/i) }
        raise 'footer lost after returning' unless chat.call(tui.screen).any? { |line| line.strip == summary }
        raise 'draft submitted on return' unless provider.requests.length == 2
        raise 'draft lost on return' unless text.call(tui.screen).include?('UNSENT_STATUS_DRAFT')
      end
    ensure
      File.binwrite(File.join(directory, 'terminal.ansi'), tui.raw) if tui
      tui&.close
      provider.close
      interaction_command(environment, binary, '--config', config, 'daemon', 'stop')
      report[:cleanup] << {mode: mode, private_daemon_stop_succeeded: true}
    end
  end
  raise 'binary changed during verification' unless Digest::SHA256.file(binary).hexdigest == report[:binary_sha256]
  report[:passed] = true
rescue StandardError => error
  report[:error] = "#{error.class}: #{error.message}"
  raise
ensure
  report[:checks_passed] = report[:checks].count { |check| check[:passed] }
  File.write(File.join(output, 'status-symbols.json'), JSON.pretty_generate(report))
  checks = report[:checks].map { |check| "- #{check[:mode]} / #{check[:language]} / #{check[:name]}：#{check[:passed] ? 'PASS' : 'FAIL'}" }
  File.write(File.join(output, 'status-symbols.md'), "# TUI 状态符号与点击验收\n\n#{report[:version]}\n\n#{checks.join("\n")}\n\n结果：#{report[:passed] ? 'PASS' : 'FAIL'}。#{report[:error]}\n\n#{report[:scope]}\n\n#{report[:isolation]}\n")
  FileUtils.remove_entry(root) if File.directory?(root)
end
