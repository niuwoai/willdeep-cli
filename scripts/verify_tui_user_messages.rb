#!/usr/bin/env ruby
# frozen_string_literal: true

# Actual TUI + Runtime with a loopback-only provider and an isolated home.
# ruby scripts/verify_tui_user_messages.rb BINARY OUTPUT_DIRECTORY
require 'json'
require 'socket'
require 'tmpdir'
require 'fileutils'
require 'pty'
require 'io/console'
require 'open3'
require 'timeout'
require 'cgi'

binary = File.expand_path(ARGV.fetch(0))
output = File.expand_path(ARGV.fetch(1))
root = Dir.mktmpdir('wd-user-blocks-')
home, workspace = %w[home workspace].map { |name| File.join(root, name) }
FileUtils.mkdir_p([home, workspace, output])
server = TCPServer.new('127.0.0.1', 0)
connections, workers, requests, checks = [], [], [], []
config = File.join(home, 'config.toml')
File.write(config, <<~TOML, perm: 0o600)
  version = 1
  default_provider = 'fixture'
  [agent]
  approval = 'smart'
  max_turns = 2
  auto_title = false
  input_suggestions = false
  small_model_routing = false
  auto_dispatch_read_only = false
  [ui]
  mode = 'compact'
  check_updates = false
  [providers.fixture]
  provider = 'openai-compatible'
  api = 'chat-completions'
  api_base = 'http://127.0.0.1:#{server.addr[1]}/v1'
  api_key = 'fixture-only'
  model = 'fixture-chat-model'
TOML
listener = Thread.new do
  loop do
    client = server.accept
    connections << client
    workers << Thread.new(client) do |socket|
      begin
        method, path = socket.gets.to_s.split
        headers = {}
        while (line = socket.gets) && line != "\r\n"
          name, value = line.split(':', 2)
          headers[name.downcase] = value.strip if value
        end
        body = socket.read(headers.fetch('content-length', '0').to_i)
        if method == 'POST' && path.end_with?('/chat/completions')
          request = JSON.parse(body)
          requests << { model: request['model'] }
          answer = requests.size == 1 ? '可以。把代码贴过来，我会先检查关键逻辑。' : '已调整：用户消息靠右显示，只保留发送时间；模型回复在左侧，与用户消息之间留一行空白。'
          payload = { model: 'fixture-chat-model', choices: [{ index: 0, delta: { content: answer }, finish_reason: 'stop' }], usage: { prompt_tokens: 30, completion_tokens: 20, total_tokens: 50 } }
          socket.write("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: #{JSON.generate(payload)}\n\ndata: [DONE]\n\n")
        else
          socket.write("HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        end
      rescue IOError, Errno::EPIPE, Errno::ECONNRESET => error
        warn "fixture connection closed: #{error.class}"
      ensure
        socket.close unless socket.closed?
      end
    end
  end
rescue IOError, Errno::EBADF
  # Listener closes during fixture cleanup.
end

env = { 'WILLDEEP_HOME' => home, 'TERM' => 'xterm-256color', 'COLORTERM' => 'truecolor',
        'WILLDEEP_TELEMETRY_DISABLED' => '1', 'WILLDEEP_LOCAL_DISCOVERY' => '0',
        'WILLDEEP_NO_UPDATE_CHECK' => '1', 'NO_PROXY' => '127.0.0.1,localhost',
        'no_proxy' => '127.0.0.1,localhost', 'SSH_CONNECTION' => 'fixture 1 fixture 2' }
%w[WILLDEEP_API_BASE WILLDEEP_API_KEY WILLDEEP_MODEL WILLDEEP_CONFIG OPENAI_BASE_URL OPENAI_API_KEY ANTHROPIC_BASE_URL ANTHROPIC_API_KEY].each { |name| env[name] = nil }
bytes = ''.b
cols, rows = 120, 35
snapshot_js = <<~'JS'
  const fs = require('node:fs');
  const { createRequire } = require('node:module');
  const { Terminal } = createRequire(process.argv[1])('@xterm/headless');
  const term = new Terminal({ cols: +process.argv[3], rows: +process.argv[4], allowProposedApi: true });
  term.write(fs.readFileSync(process.argv[2]), () => {
    const cells = [], lines = [];
    for (let y = 0; y < term.rows; y++) {
      const line = term.buffer.active.getLine(y);
      lines.push(line.translateToString(true));
      for (let x = 0; x < term.cols; x++) {
        const c = line.getCell(x);
        cells.push({ x, y, text: c.getChars(), width: c.getWidth(), bg: c.isBgRGB() ? c.getBgColor() : null,
          fg: c.isFgRGB() ? '#' + c.getFgColor().toString(16).padStart(6, '0') : null,
          palette: c.isFgPalette() ? c.getFgColor() : null });
      }
    }
    process.stdout.write(JSON.stringify({ lines, cells }));
    term.dispose();
  });
JS
snapshot = lambda do
  path = File.join(root, 'screen.ansi')
  File.binwrite(path, bytes)
  out, error, status = Open3.capture3('node', '-e', snapshot_js,
    File.expand_path('../web/package.json', __dir__), path, cols.to_s, rows.to_s)
  raise "terminal snapshot failed: #{error}" unless status.success?
  JSON.parse(out.force_encoding(Encoding::UTF_8))
end
render_svg = lambda do |screen, name|
  cell_width, cell_height = 10, 21
  palette = %w[#151b28 #e06c75 #98c379 #e5c07b #61afef #c678dd #56b6c2 #c0c7d2 #596273 #ee8589 #afe08b #f4d17c #81bffa #d99bee #78d1dc #ffffff]
  svg = [%( <svg xmlns="http://www.w3.org/2000/svg" width="#{cols * cell_width + 32}" height="#{rows * cell_height + 32}" viewBox="0 0 #{cols * cell_width + 32} #{rows * cell_height + 32}">),
    %(<rect width="100%" height="100%" fill="#1b2232"/>),
    %(<g font-family="'SFMono-Regular', Menlo, 'PingFang SC', monospace" font-size="16">)]
  screen.fetch('cells').each do |cell|
    x, y = 16 + cell['x'] * cell_width, 16 + cell['y'] * cell_height
    svg << %(<rect x="#{x}" y="#{y}" width="#{cell_width}" height="#{cell_height}" fill="##{cell['bg'].to_s(16).rjust(6, '0')}"/>) if cell['bg']
    next if cell['text'].empty? || cell['text'] == ' ' || cell['width'].zero?
    color = cell['fg'] || palette.fetch(cell['palette'] || 7, '#c0c7d2')
    svg << %(<text x="#{x}" y="#{y + 16}" fill="#{color}">#{CGI.escapeHTML(cell['text'])}</text>)
  end
  svg << '</g></svg>'
  File.write(File.join(output, "#{name}.svg"), svg.join("\n"))
end

begin
  PTY.spawn(env, binary, '--config', config, '--workspace', workspace, '--language', 'en', '--new-session') do |reader, writer, pid|
    reader.winsize = [rows, cols]
    pump = lambda do
      next unless IO.select([reader], nil, nil, 0.1)
      part = reader.read_nonblock(1_000_000, exception: false)
      raise 'TUI exited unexpectedly' if part.nil?
      if part.is_a?(String)
        bytes << part
        writer.write("\e[6;18;9t\e[0n") if part.include?("\e[16t") || part.include?("\e[5n")
      end
    end
    wait = lambda do |name, &condition|
      deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + 35
      loop do
        pump.call
        screen = snapshot.call
        break screen if condition.call(screen)
        raise "#{name}: timed out" if Process.clock_gettime(Process::CLOCK_MONOTONIC) > deadline
      end
    end
    verify = lambda do |screen, name|
      user_lines = screen.fetch('lines').each_index.select { |i| screen['lines'][i].include?('You:') }
      raise "#{name}: user messages missing" if user_lines.empty?
      user_lines.each do |y|
        block = screen['cells'].select { |cell| cell['y'] == y && cell['bg'] == 0x24303e }
        raise "#{name}: background missing" if block.empty?
        raise "#{name}: user block did not move right" unless block.first['x'] > 2
      end
      block_rows = screen['cells'].select { |cell| cell['bg'] == 0x24303e }.map { |cell| cell['y'] }.uniq
      raise "#{name}: user shows model" if block_rows.any? { |y| screen['lines'][y].include?('fixture-chat-model') }
      reply = screen['lines'].index { |line| line.include?('已调整') }
      raise "#{name}: assistant reply missing" unless reply
      raise "#{name}: role gap missing" unless screen['lines'][reply - 1].delete('│').strip.empty?
      raise "#{name}: assistant model missing" unless screen['lines'].any? { |line| line.include?('fixture-chat-model') }
      checks << { name: name, passed: true, columns: cols, rows: rows }
      File.write(File.join(output, "#{name}.txt"), screen['lines'].join("\n"))
      render_svg.call(screen, name)
    end
    begin
      wait.call('startup') { |screen| screen['lines'].any? { |line| line.include?('Hello,') } }
      writer.write("帮我看一下这段代码。\r")
      wait.call('first reply') { |screen| screen['lines'].any? { |line| line.include?('把代码贴过来') } }
      writer.write("用户消息只显示发送时间，不要标注模型名。背景淡一点，整体靠右，回复之间留出空白。\r")
      compact = wait.call('second reply') { |screen| screen['lines'].any? { |line| line.include?('已调整') } }
      verify.call(compact, 'compact')
      writer.write("/uistyle full\r")
      full = wait.call('full layout') { |screen| screen['lines'].any? { |line| line.include?('Activity') } }
      verify.call(full, 'full')
      writer.write("/uistyle compact\r")
      wait.call('compact layout') { |screen| screen['lines'].none? { |line| line.include?('Activity') } }
      cols, rows = 60, 35
      reader.winsize = [rows, cols]
      narrow = wait.call('narrow layout') { |screen| screen['lines'].any? { |line| line.include?('已调整') } && screen['cells'].any? { |cell| cell['bg'] == 0x24303e && cell['x'] < 50 } }
      verify.call(narrow, 'narrow')
      writer.write("/exit\r")
    ensure
      File.binwrite(File.join(output, 'terminal.ansi'), bytes)
      Process.kill('TERM', pid) rescue Errno::ESRCH
      begin
        Timeout.timeout(3) { Process.wait(pid) }
      rescue Timeout::Error
        Process.kill('KILL', pid) rescue Errno::ESRCH
      rescue Errno::ECHILD
        # Reaped by PTY.
      end
    end
  end
  raise "unexpected model request count: #{requests.size}" unless requests.size == 2
  report = { passed: true, version: '0.149.0-rc7', checks: checks, model_requests: requests.size,
    isolation: 'Temporary WILLDEEP_HOME, loopback provider; no real credentials or model calls.',
    previews: 'SVG reconstructed from the actual TUI ANSI cells; terminal font/palette may differ.' }
  File.write(File.join(output, 'integration.json'), JSON.pretty_generate(report))
  File.write(File.join(output, 'integration.md'), "# TUI 用户消息验收\n\n版本：0.149.0-rc7。真实 TUI + Runtime，独立临时 home，仅本机模拟模型。\n\n" + checks.map { |check| "- #{check[:name]}（#{check[:columns]} 列）：PASS" }.join("\n") + "\n\n验证用户模型过滤、右侧背景块、角色间距、助手模型保留。SVG 从实际 ANSI 单元格还原，字体与终端调色板可能不同。\n")
  puts JSON.generate(report)
ensure
  Open3.capture3(env, binary, 'daemon', 'stop')
  server.close
  listener.join(2)
  listener.kill if listener.alive?
  connections.each { |client| client.close unless client.closed? }
  workers.each { |worker| worker.join(1); worker.kill if worker.alive? }
end
