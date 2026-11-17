#!/usr/bin/env ruby
# frozen_string_literal: true

# Real PTY verification; isolated home, no model requests or system clipboard writes.
# ruby scripts/verify_tui_prompt_selection.rb BINARY OUTPUT_DIRECTORY [--ssh-copy]
# --ssh-copy requires a binary rebuilt with the SSH-aware prompt clipboard route;
# omit it for baseline binaries, whose clipboard behavior is not isolated.
require 'json'
require 'digest'
require 'base64'
require 'tmpdir'
require 'fileutils'
require 'open3'
require 'pty'
require 'io/console'
require 'timeout'

COLUMNS = 80
ROWS = 30
DRAFT = "草稿😀 Alpha\n\n  Beta  "
DECODER = <<~'JS'
  const fs = require('node:fs');
  const { createRequire } = require('node:module');
  const { Terminal } = createRequire(process.argv[1])('@xterm/headless');
  const term = new Terminal({ cols: 80, rows: 30, allowProposedApi: true });
  // xterm's Unicode 6 default treats the fixture's smile as one cell;
  // Rust unicode-width and modern terminals render it as two cells.
  const base = term._core.unicodeService._activeProvider;
  const fixtureUnicode = Object.create(base);
  fixtureUnicode.version = 'fixture-emoji-width';
  fixtureUnicode.wcwidth = codepoint => codepoint === 0x1f600 ? 2 : base.wcwidth(codepoint);
  term.unicode.register(fixtureUnicode);
  term.unicode.activeVersion = fixtureUnicode.version;
  term.write(fs.readFileSync(process.argv[2]), () => {
    const cells = [], lines = [];
    for (let y = 0; y < term.rows; y++) {
      const line = term.buffer.active.getLine(term.buffer.active.viewportY + y);
      lines.push(line.translateToString(true));
      for (let x = 0; x < term.cols; x++) {
        const cell = line.getCell(x);
        cells.push({ x, y, text: cell.getChars(), bg: cell.getBgColor(), fg: cell.getFgColor(),
          inverse: cell.isInverse() });
      }
    }
    process.stdout.write(JSON.stringify({ lines, cells }));
    term.dispose();
  });
JS

binary = File.realpath(ARGV.fetch(0))
output = File.expand_path(ARGV.fetch(1))
ssh_copy = ARGV.drop(2).include?('--ssh-copy')
FileUtils.mkdir_p(output)
root = Dir.mktmpdir('wd-prompt-selection-')
environment = {
  'PATH' => ENV.fetch('PATH'), 'HOME' => File.join(root, 'user'),
  'TERM' => 'xterm-256color', 'COLORTERM' => 'truecolor', 'LANG' => 'en_US.UTF-8',
  'WILLDEEP_TELEMETRY_DISABLED' => '1', 'WILLDEEP_NO_UPDATE_CHECK' => '1',
  'WILLDEEP_LOCAL_DISCOVERY' => '0', 'DO_NOT_TRACK' => '1',
  'NO_PROXY' => '127.0.0.1,localhost', 'no_proxy' => '127.0.0.1,localhost'
}
checks = []
report = { passed: false, checks: checks, ssh_copy: ssh_copy,
  binary_sha256: Digest::SHA256.file(binary).hexdigest,
  isolation: 'Temporary HOME/WILLDEEP_HOME; copy_on_select=false; copy key only in isolated SSH PTYs; OSC 52 captured without a desktop clipboard; no prompt submission.' }
version, error, status = Open3.capture3(environment, binary, '--version', unsetenv_others: true)
raise "version failed: #{error}" unless status.success?
report[:version] = version.strip

begin
  modes = %w[compact full]
  modes += %w[compact-ssh full-ssh] if ssh_copy
  modes.each do |mode|
    remote = mode.end_with?('-ssh')
    layout = mode.delete_suffix('-ssh')
    home = File.join(root, mode, 'home')
    workspace = File.join(root, mode, 'workspace')
    FileUtils.mkdir_p([home, workspace])
    env = environment.merge('WILLDEEP_HOME' => home)
    env['SSH_CONNECTION'] = 'fixture 1 fixture 2' if remote
    config = File.join(home, 'config.toml')
    lines = ['version = 1', "default_provider = 'fixture'", '[agent]', "approval = 'smart'",
      'auto_title = false', 'input_suggestions = false', 'small_model_routing = false',
      'auto_dispatch_read_only = false', '[ui]', "mode = '#{layout}'", 'check_updates = false',
      'copy_on_select = false', '[providers.fixture]', "provider = 'openai-compatible'",
      "api = 'chat-completions'", "api_base = 'http://127.0.0.1:9/v1'",
      "api_key = 'fixture-only'", "model = 'fixture'"]
    File.write(config, lines.join("\n") + "\n", perm: 0o600)
    raw = ''.b
    snapshot_path = File.join(output, "#{mode}.ansi")
    begin
      PTY.spawn(env, binary, '--config', config, '--workspace', workspace,
        '--language', 'en', '--new-session', unsetenv_others: true) do |reader, writer, pid|
        reader.winsize = [ROWS, COLUMNS]
        pump = lambda do
          next unless IO.select([reader], nil, nil, 0.03)
          data = reader.read_nonblock(1_000_000, exception: false)
          raise "#{mode}: TUI exited" if data.nil?
          next unless data.is_a?(String)
          raw << data
          writer.write("\e[6;18;9t\e[0n") if data.include?("\e[16t") || data.include?("\e[5n")
        end
        snapshot = lambda do
          File.binwrite(snapshot_path, raw)
          result, decoder_error, decoder_status = Open3.capture3('node', '-e', DECODER,
            File.expand_path('../web/package.json', __dir__), snapshot_path)
          raise "terminal decoder failed: #{decoder_error}" unless decoder_status.success?
          JSON.parse(result.force_encoding(Encoding::UTF_8))
        end
        wait = lambda do |label, &condition|
          deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + 25
          loop do
            pump.call
            screen = snapshot.call
            break screen if condition.call(screen)
            raise "#{mode}: #{label} timed out" if Process.clock_gettime(Process::CLOCK_MONOTONIC) > deadline
          end
        end
        drag = lambda do |start_cell, end_cell|
          writer.write("\e[<0;#{start_cell['x'] + 1};#{start_cell['y'] + 1}M")
          writer.write("\e[<32;#{end_cell['x'] + 1};#{end_cell['y'] + 1}M")
          writer.write("\e[<0;#{end_cell['x'] + 1};#{end_cell['y'] + 1}m")
        end
        begin
          wait.call('startup') { |screen| screen['lines'].any? { |line| line.include?('Hello,') } }
          writer.write("\e[200~#{DRAFT}\e[201~")
          initial = wait.call('draft') do |screen|
            screen['lines'].any? { |line| line.include?('草稿😀 Alpha') } &&
              screen['lines'].any? { |line| line.include?('Beta') }
          end
          first = initial['cells'].find { |cell| cell['text'] == '草' }
          raise "#{mode}: draft start missing" unless first
          last = initial['cells'].find { |cell| cell['y'] > first['y'] && cell['text'] == 'B' }
          raise "#{mode}: draft end missing" unless last
          drag.call(first, last)
          highlighted = wait.call('cross-line highlight') do |screen|
            cell = screen['cells'].find { |item| item['x'] == first['x'] && item['y'] == first['y'] }
            [cell['bg'], cell['fg'], cell['inverse']] != [first['bg'], first['fg'], first['inverse']]
          end
          raise "#{mode}: selection unexpectedly quoted" if highlighted['lines'].any? { |line| line.include?('quote') && line.include?('selected') }
          File.write(File.join(output, "#{mode}-selection.json"), JSON.pretty_generate(highlighted))
          checks << { mode: mode, name: 'Chinese, emoji and cross-line selection visibly highlighted', passed: true }
          writer.write("\e")
          wait.call('Esc clears highlight and preserves draft') do |screen|
            cell = screen['cells'].find { |item| item['x'] == first['x'] && item['y'] == first['y'] }
            [cell['bg'], cell['fg'], cell['inverse']] == [first['bg'], first['fg'], first['inverse']] &&
              screen['lines'].any? { |line| line.include?('草稿😀 Alpha') } &&
              screen['lines'].any? { |line| line.include?('Beta') }
          end
          checks << { mode: mode, name: 'Esc clears selection and preserves unsent draft', passed: true }
          drag.call(first, last)
          wait.call('second selection') do |screen|
            cell = screen['cells'].find { |item| item['x'] == first['x'] && item['y'] == first['y'] }
            cell['bg'] != first['bg'] || cell['inverse'] != first['inverse']
          end
          if remote
            # SSH forces the terminal route, so Ctrl+C cannot touch arboard.
            # The PTY captures OSC 52; it is never forwarded to a real terminal.
            copy_offset = raw.bytesize
            writer.write("\x03")
            copied = wait.call('Ctrl+C sends original draft through OSC 52 without exiting') do |screen|
              encoded = raw.byteslice(copy_offset..).scan(/\e\]52;c;([A-Za-z0-9+\/=]*)\a/n).last&.first
              next false unless encoded
              payload = Base64.strict_decode64(encoded).force_encoding(Encoding::UTF_8)
              raise "#{mode}: copied payload changed: #{payload.inspect}" unless payload == "草稿😀 Alpha\n\n  "
              screen['lines'].any? { |line| line.include?('草稿😀 Alpha') } &&
                screen['lines'].any? { |line| line.include?('Beta') } &&
                screen['lines'].any? { |line| line.include?('Sent') && line.include?('prompt chars') }
            end
            File.write(File.join(output, "#{mode}-copy.json"), JSON.pretty_generate(copied))
            checks << { mode: mode, name: 'Ctrl+C sends exact original selection through OSC 52 and keeps draft alive', passed: true }
          end
          writer.write('yq')
          typed = wait.call('normal y/q input after selection') do |screen|
            screen['lines'].any? { |line| line.include?('yq') } &&
              screen['lines'].any? { |line| line.include?('草稿') }
          end
          File.write(File.join(output, "#{mode}-typing.json"), JSON.pretty_generate(typed))
          checks << { mode: mode, name: 'y/q are typed into draft instead of chat selection commands', passed: true }
        ensure
          File.binwrite(snapshot_path, raw)
          Process.kill('TERM', pid) rescue Errno::ESRCH
          reader.close unless reader.closed?
          writer.close unless writer.closed?
          begin
            Timeout.timeout(3) { Process.wait(pid) }
          rescue Timeout::Error
            Process.kill('KILL', pid) rescue Errno::ESRCH
            Process.detach(pid) rescue Errno::ECHILD
          rescue Errno::ECHILD
            # Reaped by PTY.
          end
        end
      end
    ensure
      _, stop_error, stop_status = Open3.capture3(env, binary, 'daemon', 'stop', unsetenv_others: true)
      warn "#{mode}: fixture daemon cleanup failed: #{stop_error}" unless stop_status.success?
    end
  end
  raise 'binary changed during prompt selection verification' unless Digest::SHA256.file(binary).hexdigest == report[:binary_sha256]
  report[:passed] = true
rescue StandardError => failure
  report[:error] = "#{failure.class}: #{failure.message}"
  raise
ensure
  File.write(File.join(output, 'integration.json'), JSON.pretty_generate(report))
  clipboard_scope = if ssh_copy
    'SSH 夹具直接截获 OSC 52 并核对原文 payload 与已发送提示'
  else
    '未发送复制快捷键；SSH 复制场景未启用'
  end
  File.write(File.join(output, 'integration.md'), "# TUI 输入区拖选验收\n\n#{report[:version]}\n\n" +
    checks.map { |check| "- #{check[:mode]} / #{check[:name]}：PASS" }.join("\n") +
    "\n\n结果：#{report[:passed] ? 'PASS' : 'FAIL'}。#{report[:error]}\n\n独立临时配置，未提交提示词，未写入系统剪贴板；#{clipboard_scope}，弹层优先级与其余复制决策由 Rust 单元测试验证。\n")
  puts JSON.generate(report)
end
