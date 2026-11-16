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

CELL_WIDTH = 10
CELL_HEIGHT = 20
WHALE_ROWS = 7
CAPTURE_ROWS = 45
PROBE_ROWS = 80
PROBE_COLUMNS = [120, 60, 40, 26, 25, 24, 23, 14, 13, 10, 120].freeze
BLOCK_GLYPHS = /[▀▄█]/
# Golden glyph rows, independent of the runtime renderer: assert every cell, not just seven occupied rows.
WIDE_WHALE = [
  '            ▄▄', '     ▄▄    ▄██▄', '   ▄█████████████▄▄',
  '  ██████████████████▄', '  ████████████████████▄',
  '   ▀██████████████████▀▀', '     ▀▀▀████████▀▀▀'
].map { |row| row.ljust(24) }.freeze
SMALL_WHALE = [
  '     ▄▄', '    ▄▀▀▄', ' ▄████████▄', '████████████',
  '████████████', ' ▀████████▀', '   ▀████▀'
].map { |row| row.ljust(12) }.freeze

binary = File.expand_path(ARGV.fetch(0))
version_output, version_error, version_status = Open3.capture3(binary, '--version')
abort "cannot read TUI binary version: #{version_error}" unless version_status.success?
version = version_output[/\Awilldeep (\d+\.\d+\.\d+-rc\d+)\s*\z/, 1]
abort "unexpected TUI version output: #{version_output.inspect}" unless version
output = File.expand_path(ARGV.fetch(1))
root = Dir.mktmpdir('wd-user-blocks-')
home, workspace = %w[home workspace].map { |name| File.join(root, name) }
FileUtils.mkdir_p([home, workspace, output])
server = TCPServer.new('127.0.0.1', 0)
connections, workers, requests, checks, whale_checks = [], [], [], [], []
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
          answer = requests.size == 1 ? '可以。把代码贴过来，我会先检查关键逻辑。' : '我会先检查输入边界、异常分支和资源释放，再说明发现的问题与修改建议。'
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
cols, rows = 120, CAPTURE_ROWS
resize_events = [{ offset: 0, cols: cols, rows: rows }]
snapshot_js = <<~'JS'
  const fs = require('node:fs');
  const { createRequire } = require('node:module');
  const { Terminal } = createRequire(process.argv[1])('@xterm/headless');
  const sizes = JSON.parse(process.argv[3]);
  const term = new Terminal({ cols: sizes[0].cols, rows: sizes[0].rows, allowProposedApi: true });
  const bytes = fs.readFileSync(process.argv[2]);
  (async () => {
    for (let i = 0; i < sizes.length; i++) {
      const size = sizes[i];
      term.resize(size.cols, size.rows);
      const segment = bytes.subarray(size.offset, sizes[i + 1]?.offset ?? bytes.length);
      await new Promise(resolve => term.write(segment, resolve));
    }
    const cells = [], lines = [], wrapped = [];
    for (let y = 0; y < term.rows; y++) {
      const line = term.buffer.active.getLine(y);
      lines.push(line.translateToString(true));
      wrapped.push(line.isWrapped);
      for (let x = 0; x < term.cols; x++) {
        const c = line.getCell(x);
        cells.push({ x, y, text: c.getChars(), width: c.getWidth(), bg: c.isBgRGB() ? c.getBgColor() : null,
          fg: c.isFgRGB() ? '#' + c.getFgColor().toString(16).padStart(6, '0') : null,
          palette: c.isFgPalette() ? c.getFgColor() : null });
      }
    }
    process.stdout.write(JSON.stringify({ lines, cells, wrapped, cols: term.cols, rows: term.rows }));
    term.dispose();
  })().catch(error => { console.error(error); process.exitCode = 1; });
JS
snapshot = lambda do
  path = File.join(root, 'screen.ansi')
  File.binwrite(path, bytes)
  out, error, status = Open3.capture3('node', '-e', snapshot_js,
    File.expand_path('../web/package.json', __dir__), path, JSON.generate(resize_events))
  raise "terminal snapshot failed: #{error}" unless status.success?
  JSON.parse(out.force_encoding(Encoding::UTF_8))
end
render_svg = lambda do |screen, name|
  palette = %w[#151b28 #e06c75 #98c379 #e5c07b #61afef #c678dd #56b6c2 #c0c7d2 #596273 #ee8589 #afe08b #f4d17c #81bffa #d99bee #78d1dc #ffffff]
  svg = [%( <svg xmlns="http://www.w3.org/2000/svg" width="#{cols * CELL_WIDTH + 32}" height="#{rows * CELL_HEIGHT + 32}" viewBox="0 0 #{cols * CELL_WIDTH + 32} #{rows * CELL_HEIGHT + 32}">),
    %(<rect width="100%" height="100%" fill="#1b2232"/>),
    %(<g font-family="'SFMono-Regular', Menlo, 'PingFang SC', monospace" font-size="16">)]
  # Paint backgrounds first: a wide glyph's continuation cell must not cover its right half.
  screen.fetch('cells').each do |cell|
    next unless cell['bg']
    x, y = 16 + cell['x'] * CELL_WIDTH, 16 + cell['y'] * CELL_HEIGHT
    svg << %(<rect x="#{x}" y="#{y}" width="#{CELL_WIDTH}" height="#{CELL_HEIGHT}" fill="##{cell['bg'].to_s(16).rjust(6, '0')}"/>)
  end
  screen.fetch('cells').each do |cell|
    x, y = 16 + cell['x'] * CELL_WIDTH, 16 + cell['y'] * CELL_HEIGHT
    next if cell['text'].empty? || cell['text'] == ' ' || cell['width'].zero?
    color = cell['fg'] || palette.fetch(cell['palette'] || 7, '#c0c7d2')
    # Block glyphs describe cell geometry, not font outlines. Each half is a square pixel.
    case cell['text']
    when '█'
      svg << %(<rect data-cell-block="full" x="#{x}" y="#{y}" width="#{CELL_WIDTH}" height="#{CELL_HEIGHT}" fill="#{color}"/>)
    when '▀', '▄'
      half = CELL_HEIGHT / 2
      top = cell['text'] == '▀' ? y : y + half
      svg << %(<rect data-cell-block="half" x="#{x}" y="#{top}" width="#{CELL_WIDTH}" height="#{half}" fill="#{color}"/>)
    else
      svg << %(<text x="#{x}" y="#{y + 16}" fill="#{color}">#{CGI.escapeHTML(cell['text'])}</text>)
    end
  end
  svg << '</g></svg>'
  raise "#{name}: block glyph rendered through a font" if svg.any? { |line| line.match?(/<text[^>]*>[▀▄█]<\/text>/) }
  File.write(File.join(output, "#{name}.svg"), svg.join("\n"))
end
whale_visible = lambda do |screen|
  title = screen.fetch('lines').index { |line| line.include?('WILLDEEP') }
  next false unless title && title >= WHALE_ROWS + 1
  whale = screen.fetch('cells').select { |cell| cell['text'].match?(BLOCK_GLYPHS) }
  expected_rows = ((title - WHALE_ROWS - 1)...(title - 1)).to_a
  content_width = screen.fetch('cols') - 2
  pattern = content_width >= 24 ? WIDE_WHALE : SMALL_WHALE
  if content_width < 12
    pattern = pattern.map do |row|
      Array.new(content_width) { |x| row[((x + 0.5) * 12 / content_width).floor] }.join
    end
  end
  whale.map { |cell| cell['y'] }.uniq == expected_rows &&
    expected_rows.none? { |y| screen.fetch('wrapped')[y] } &&
    whale.all? { |cell| cell['x'] >= 1 && cell['x'] < screen.fetch('cols') - 1 } &&
    expected_rows.each_with_index.all? do |y, index|
      actual = screen.fetch('cells').select { |cell| cell['y'] == y && cell['x'].between?(1, content_width) }
        .map { |cell| cell['text'].empty? ? ' ' : cell['text'] }.join
      actual == pattern.fetch(index).ljust(content_width)
    end
end

begin
  %w[messages whale-widths].each do |capture|
    bytes = ''.b
    cols, rows = 120, capture == 'messages' ? CAPTURE_ROWS : PROBE_ROWS
    resize_events = [{ offset: 0, cols: cols, rows: rows }]
    PTY.spawn(env, binary, '--config', config, '--workspace', workspace, '--language', 'en', '--new-session') do |reader, writer, pid|
      reader.winsize = [rows, cols]
      pump = lambda do
        next unless IO.select([reader], nil, nil, 0.1)
        part = reader.read_nonblock(1_000_000, exception: false)
        raise 'TUI exited unexpectedly' if part.nil?
        if part.is_a?(String)
          bytes << part
          writer.write("\e[6;#{CELL_HEIGHT};#{CELL_WIDTH}t\e[0n") if part.include?("\e[16t") || part.include?("\e[5n")
        end
      end
      wait = lambda do |name, &condition|
        deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + 35
        matched_since = nil
        matched_screen = nil
        loop do
          pump.call
          screen = snapshot.call
          now = Process.clock_gettime(Process::CLOCK_MONOTONIC)
          if condition.call(screen)
            # Markers can appear in a partial redraw; require the whole screen to settle.
            matched_since = now unless screen == matched_screen
            matched_screen = screen
            break screen if now - matched_since >= 0.5
          else
            matched_since = nil
            matched_screen = nil
          end
          raise "#{name}: timed out" if now > deadline
        end
      end
      verify = lambda do |screen, name|
        user_lines = screen.fetch('cells').select { |cell| cell['bg'] == 0x24303e }.map { |cell| cell['y'] }.uniq
        full_layout = screen['lines'].any? { |line| line.include?('Activity') }
        raise "#{name}: incorrect layout captured" unless full_layout == (name == 'full')
        raise "#{name}: pending layout command visible" if screen['lines'].any? { |line| line.include?('/uistyle ') }
        raise "#{name}: first reply repeated or missing" unless screen['lines'].count { |line| line.include?('把代码贴过来') } == 1
        raise "#{name}: user messages missing" if user_lines.empty?
        raise "#{name}: complete whale missing or reflowed" unless whale_visible.call(screen)
        raise "#{name}: product version missing" unless screen['lines'].any? { |line| line.include?("WILLDEEP  v#{version}") }
        raise "#{name}: generated user prefix visible" if user_lines.any? { |y| screen['lines'][y].include?('You:') }
        raise "#{name}: user text missing" unless user_lines.any? { |y| screen['lines'][y].include?('请重点检查') }
        raise "#{name}: user time missing" unless user_lines.any? { |y| screen['lines'][y].match?(/\b(?:now|\d+[smhd] ago)\b/) }
        user_lines.each do |y|
          block = screen['cells'].select { |cell| cell['y'] == y && cell['bg'] == 0x24303e }
          raise "#{name}: user block did not move right" unless block.first['x'] > 2
        end
        raise "#{name}: user shows model" if user_lines.any? { |y| screen['lines'][y].include?('fixture-chat-model') }
        reply = screen['lines'].index { |line| line.include?('我会先检查输入边界') }
        raise "#{name}: assistant reply missing" unless reply
        raise "#{name}: role gap missing" unless screen['lines'][reply - 1].delete('│').strip.empty?
        raise "#{name}: assistant model missing" unless screen['lines'].any? { |line| line.include?('fixture-chat-model') }
        checks << { name: name, passed: true, columns: cols, rows: rows, complete_whale: true }
        File.write(File.join(output, "#{name}.txt"), screen['lines'].join("\n"))
        render_svg.call(screen, name)
      end
      begin
        wait.call('startup') { |screen| screen['lines'].any? { |line| line.include?('Hello,') } && whale_visible.call(screen) }
        if capture == 'messages'
          writer.write("帮我看一下这段代码。\r")
          wait.call('first reply') { |screen| screen['lines'].any? { |line| line.include?('把代码贴过来') } }
          writer.write("请重点检查输入校验和错误处理。发现问题时，先说明原因，再给出修改建议。\r")
          compact = wait.call('second reply') do |screen|
            screen['lines'].any? { |line| line.include?('我会先检查输入边界') } &&
              screen['lines'].count { |line| line.include?('fixture-chat-model') } >= 2 &&
              screen['lines'].count { |line| line.include?('turn finished') } >= 2
          end
          verify.call(compact, 'compact')
          writer.write("/uistyle full\r")
          full = wait.call('full layout') { |screen| screen['lines'].any? { |line| line.include?('Activity') } }
          verify.call(full, 'full')
          writer.write("/uistyle compact\r")
          wait.call('compact layout') do |screen|
            screen['lines'].any? { |line| line.include?('Switched to compact UI') } &&
              screen['lines'].none? { |line| line.include?('Activity') || line.include?('/uistyle compact') }
          end
          cols = 60
          resize_events << { offset: bytes.bytesize, cols: cols, rows: rows }
          reader.winsize = [rows, cols]
          narrow = wait.call('narrow layout') do |screen|
            screen['lines'].any? { |line| line.include?('我会先检查输入边界') } &&
              screen['lines'].any? { |line| line.include?('Focus:') } &&
              whale_visible.call(screen) &&
              screen['cells'].any? { |cell| cell['bg'] == 0x24303e && cell['x'] < 50 }
          end
          verify.call(narrow, 'narrow')
        else
          %w[compact full].each do |layout|
            if layout == 'full'
              writer.write("/uistyle full\r")
              wait.call('probe full layout') { |screen| screen['lines'].any? { |line| line.include?('Activity') } }
            end
            PROBE_COLUMNS.each_with_index do |width, index|
              changed = cols != width
              redraw_offset = bytes.bytesize
              cols = width
              resize_events << { offset: redraw_offset, cols: cols, rows: rows }
              reader.winsize = [rows, cols]
              screen = wait.call("#{layout} whale at #{width} columns") do |frame|
                (!changed || bytes.bytesize > redraw_offset) && whale_visible.call(frame)
              end
              name = "whale-#{layout}-#{index}-#{width}"
              File.write(File.join(output, "#{name}.txt"), screen['lines'].join("\n"))
              render_svg.call(screen, name) if [26, 25, 13].include?(width)
              whale_checks << { layout: layout, columns: cols, content_columns: cols - 2, rows: rows,
                passed: true, whale_rows: WHALE_ROWS, soft_wrapped: false, restored_wide: index == PROBE_COLUMNS.size - 1 }
            end
          end
        end
        writer.write("/exit\r")
      ensure
        prefix = capture == 'messages' ? 'terminal' : 'whale-terminal'
        File.binwrite(File.join(output, "#{prefix}.ansi"), bytes)
        File.write(File.join(output, "#{prefix}-sizes.json"), JSON.pretty_generate(resize_events))
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
  end
  raise "unexpected model request count: #{requests.size}" unless requests.size == 2
  report = { passed: true, version: version, checks: checks, whale_width_checks: whale_checks, model_requests: requests.size,
    isolation: 'Temporary WILLDEEP_HOME, loopback provider; no real credentials or model calls.',
    previews: 'SVG reconstructed from actual ANSI cells; block glyphs are exact rectangles with square half-cells; text fonts/palette may differ.' }
  File.write(File.join(output, 'integration.json'), JSON.pretty_generate(report))
  File.write(File.join(output, 'integration.md'), "# TUI 鲸鱼与用户消息验收\n\n版本：#{version}。真实 TUI + Runtime，独立临时 home，仅本机模拟模型。\n\n" +
    checks.map { |check| "- #{check[:name]}（#{check[:columns]}×#{check[:rows]}）：PASS" }.join("\n") +
    "\n\n完整鲸鱼保持 7 行；简洁与完整布局各覆盖 #{PROBE_COLUMNS.join(' → ')} 列，含缩窄后恢复宽屏，共 #{whale_checks.size} 个真实终端检查。原始 ANSI 与尺寸事件一并保存。\n\n验证隐藏生成的用户角色前缀、保留正文和时间、用户模型过滤、右侧背景块、角色间距、助手模型保留。SVG 的半块像素按精确单元格矩形还原，不使用字体块字符。\n")
  puts JSON.generate(report)
ensure
  Open3.capture3(env, binary, 'daemon', 'stop')
  server.close
  listener.join(2)
  listener.kill if listener.alive?
  connections.each { |client| client.close unless client.closed? }
  workers.each { |worker| worker.join(1); worker.kill if worker.alive? }
end
