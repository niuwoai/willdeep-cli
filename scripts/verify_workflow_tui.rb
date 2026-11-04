#!/usr/bin/env ruby
# Helpers for the Runtime fixture; standalone mode attaches only to an existing isolated home.
require 'pty'
require 'io/console'
require 'json'
require 'base64'
require 'fileutils'
require 'open3'
require 'socket'
require 'net/http'
require 'digest'
require 'optparse'

WORKFLOW_TERMINAL_JS = <<~'JS'
  const { createRequire } = require('node:module');
  const { Terminal } = createRequire(process.argv[1])('@xterm/headless');
  let input = '';
  process.stdin.on('data', part => input += part);
  process.stdin.on('end', async () => {
    const { data, columns, rows } = JSON.parse(input);
    const terminal = new Terminal({ cols: columns, rows, allowProposedApi: true });
    const notices = new Set();
    // A notice may last one redraw. Render each real synchronized frame so a
    // later Ready frame does not hide the command's actual acknowledgement.
    for (const frame of Buffer.from(data, 'base64').toString('utf8').split('\x1b[?2026l')) {
      await new Promise(resolve => terminal.write(frame, resolve));
      const active = terminal.buffer.active;
      notices.add(active.getLine(active.viewportY + rows - 1)?.translateToString(true) ?? '');
    }
    const buffer = terminal.buffer.active;
    const lines = Array.from({ length: buffer.length }, (_, i) => buffer.getLine(i)?.translateToString(true) ?? '');
    process.stdout.write(JSON.stringify({ text: lines.join('\n'), lines, notices: [...notices], columns, rows }));
    terminal.dispose();
  });
JS

def workflow_fixture_env(home)
  # All credentials/config inherited from the operator are excluded, without reading them.
  cleared = ENV.keys.grep(/\A(?:WILLDEEP_|OPENAI_|ANTHROPIC_|AZURE_OPENAI_|GOOGLE_API_|GEMINI_API_|HTTP_PROXY\z|HTTPS_PROXY\z|ALL_PROXY\z|http_proxy\z|https_proxy\z|all_proxy\z)/).to_h { |key| [key, nil] }
  cleared.merge('WILLDEEP_HOME' => home, 'WILLDEEP_CONFIG' => File.join(home, 'config.toml'),
    'WILLDEEP_TELEMETRY_DISABLED' => '1', 'NO_PROXY' => '127.0.0.1,localhost', 'no_proxy' => '127.0.0.1,localhost')
end

def validate_workflow_fixture!(fixture)
  root = File.realpath(fixture.fetch('root'))
  raise 'refusing a non-isolated workflow root' unless root.start_with?('/private/tmp/wd-workflow-') && File.directory?(root)
  %w[home workspace].each do |key|
    raise "#{key} escapes the workflow fixture" unless File.realpath(fixture.fetch(key)) == File.join(root, key)
  end
  config = fixture.fetch('config', File.join(fixture.fetch('home'), 'config.toml'))
  raise 'config escapes the workflow fixture' unless File.realpath(config) == File.join(root, 'home/config.toml')
end

def write_workflow_report(output, report, stem = 'report')
  FileUtils.mkdir_p(output)
  File.write(File.join(output, "#{stem}.json"), JSON.pretty_generate(report) + "\n")
  entries = report.fetch(:checks, []).map do |entry|
    state = entry[:status] == 'skipped' ? '跳过' : (entry[:passed] ? '通过' : '失败')
    "- #{entry[:check]}：#{state}"
  end
  text = "# 工作流 Runtime 与终端验收\n\n版本：#{report[:version]}\n\n结果：#{report[:passed] ? '通过' : '失败'}。真实 Runtime、隔离 WILLDEEP_HOME、本机假模型。\n\n#{entries.join("\n")}\n"
  text += "\n共享调度默认上限 8 并发 / 64 排队；本次隔离配置实测上限 3 并发 / 2 排队，共 18 个工作流叶节点和 1 个普通 Worker。\n" if entries.any? { |entry| entry.include?('shared_backpressure') }
  text += "\n错误：#{report[:error]}\n" if report[:error]
  text += "\n终端证据来自真实 PTY 和 @xterm/headless 渲染；没有检查实体终端窗口。原始 ANSI、输入操作及渲染屏幕一并保存。\n" if entries.any? { |entry| entry.include?('pty') }
  File.write(File.join(output, "#{stem}.md"), text)
end

def workflow_public_api(binary, env, operation, params)
  out, err, status = Open3.capture3(env, binary, 'api', operation, '--params-file', '-', '--ndjson', stdin_data: JSON.generate(params))
  raise "#{operation}: #{err} #{out}" unless status.success?
  response = JSON.parse(out.force_encoding(Encoding::UTF_8))
  raise "#{operation}: #{response}" unless response['status'] == 'ok'
  response.fetch('data')
end

def workflow_wait(description, seconds = 25)
  deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + seconds
  loop do
    result = yield
    return result if result
    raise "timeout: #{description}" if Process.clock_gettime(Process::CLOCK_MONOTONIC) >= deadline
    sleep 0.1
  end
end

def workflow_finish_child(pid)
  return unless pid
  return if Process.waitpid(pid, Process::WNOHANG)
  Process.kill('TERM', pid)
  deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + 3
  loop do
    return if Process.waitpid(pid, Process::WNOHANG)
    break if Process.clock_gettime(Process::CLOCK_MONOTONIC) >= deadline
    sleep 0.05
  end
  Process.kill('KILL', pid)
  workflow_wait('terminated fixture child was reaped', 5) { Process.waitpid(pid, Process::WNOHANG) }
rescue Errno::ESRCH, Errno::ECHILD
  nil
end

def verify_workflow_projection(env, workspace, parent, run_id, output)
  # The short-lived server is owned by this check; it never attaches to port 19378.
  reservation = TCPServer.new('127.0.0.1', 0)
  port = reservation.addr[1]; reservation.close
  log = File.open(File.join(output, 'projection-web.log'), 'w')
  pid = Process.spawn(env, BIN, '--web', '--listen', "127.0.0.1:#{port}", '-w', workspace, '-c', env.fetch('WILLDEEP_CONFIG'), '--language', 'en', out: log, err: log)
  projection = workflow_wait('persisted session projection workflow card') do
    begin
      http = Net::HTTP.new('127.0.0.1', port, nil)
      http.open_timeout = http.read_timeout = 1
      response = http.get("/api/sessions/#{parent}")
      value = JSON.parse(response.body) if response.code == '200'
      value if value && value.fetch('messages').any? { |item| item['role'] == 'workflow' && item.dig('workflow', 'id') == run_id }
    rescue Errno::ECONNREFUSED, EOFError, Net::OpenTimeout, Net::ReadTimeout
      nil
    end
  end
  exported = fixture_api(env, 'session.export', id: parent).dig('core', 'messages')
  tools = exported.flat_map { |m| m.fetch('tool_calls', []) }.select { |c| %w[workflow_read workflow_control].include?(c['name']) }
  operations = tools.map { |c| [c['name'], JSON.parse(c.fetch('arguments')).fetch('operation')] }
  raise "chat tool sequence changed: #{operations}" unless operations == [['workflow_read', 'schema'], ['workflow_control', 'save'], ['workflow_control', 'run']]
  raise 'parent final reply was not persisted' unless exported.any? { |m| m['role'] == 'assistant' && m['content'].include?('CHAT_WORKFLOW_FINAL') }
  cards = projection.fetch('messages').select { |item| item['role'] == 'workflow' && item.dig('workflow', 'id') == run_id }
  raise 'projection duplicated the workflow card' unless cards.size == 1
  File.write(File.join(output, 'chat-session-projection.json'), JSON.pretty_generate(projection) + "\n")
  { check: 'chat_schema_save_run_final_background_phases_and_persisted_projection_card', passed: true, parent: parent, run: run_id, tool_sequence: operations, projected_cards: cards.size }
ensure
  workflow_finish_child(pid)
  log&.close
end

class WorkflowPty
  attr_reader :evidence, :bytes

  def initialize(reader, writer, output, columns, rows)
    @reader, @writer, @output, @columns, @rows = reader, writer, output, columns, rows
    @bytes, @evidence, @query_buffer = ''.b, [], ''.b
    @reader.winsize = [rows, columns]
  end

  def pump(seconds = 0.25, allow_exit: false)
    deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + seconds
    while Process.clock_gettime(Process::CLOCK_MONOTONIC) < deadline
      next unless IO.select([@reader], nil, nil, 0.05)
      data = @reader.read_nonblock(1_000_000, exception: false)
      if data.nil?
        return if allow_exit
        raise 'TUI exited before terminal assertions completed'
      end
      next if data == :wait_readable
      @bytes << data
      @query_buffer << data
      @query_buffer.gsub!(/\e\[(16t|5n|6n|c|>c|\?u)/) do
        response = { '16t' => "\e[6;18;9t", '5n' => "\e[0n", '6n' => "\e[1;1R", 'c' => "\e[?1;2c", '>c' => "\e[>0;0;0c", '?u' => "\e[?0u" }.fetch(Regexp.last_match(1))
        @writer.write(response)
        ''
      end
      @query_buffer = @query_buffer.byteslice(-64, 64) || @query_buffer
    end
  rescue Errno::EIO => error
    return if allow_exit
    raise "TUI PTY closed before assertions completed: #{error.message}"
  end

  def input(text, label)
    @writer.write(text); @writer.flush
    @evidence << { input: label, bytes_base64: Base64.strict_encode64(text), at: Process.clock_gettime(Process::CLOCK_MONOTONIC) }
  end

  def snapshot(label)
    pump
    package = File.expand_path('../web/package.json', __dir__)
    out, err, status = Open3.capture3('node', '-e', WORKFLOW_TERMINAL_JS, package,
      stdin_data: JSON.generate(data: Base64.strict_encode64(@bytes), columns: @columns, rows: @rows))
    raise "@xterm/headless could not render the real PTY output: #{err}" unless status.success?
    rendered = JSON.parse(out.force_encoding(Encoding::UTF_8))
    @evidence << rendered.merge('stage' => label)
    File.write(File.join(@output, "#{label}.txt"), rendered.fetch('text'))
    [rendered.fetch('text'), *rendered.fetch('notices')].join("\n")
  end

  def wait_screen(label, &condition)
    workflow_wait(label) { screen = snapshot(label); screen if condition.call(screen) }
  end

  def command(command, label)
    # A wrong final character, Left and Delete exercise editing before Enter.
    input("#{command}x\e[D\e[3~", "#{label}: type, Left, Delete")
    wait_screen("#{label}-input") { |screen| screen.gsub(/[\s│┃]/, '').include?(command.delete(' ')) }
    input("\r", "#{label}: Enter")
    wait_screen("#{label}-response") { |screen| screen.include?('Workflow updated') }
  end

  def save
    File.binwrite(File.join(@output, 'terminal.ansi'), @bytes)
    File.write(File.join(@output, 'terminal-evidence.json'), JSON.pretty_generate(@evidence) + "\n")
  end
end

def verify_workflow_tui(binary, fixture, output)
  validate_workflow_fixture!(fixture)
  binary = File.expand_path(binary)
  env = workflow_fixture_env(fixture.fetch('home')).merge('TERM' => 'xterm-256color', 'TERM_PROGRAM' => 'workflow-fixture', 'TMUX' => nil)
  parent = fixture.fetch('tui_parent', fixture.fetch('parent'))
  runs = workflow_public_api(binary, env, 'workflow.history', parent_session_id: parent).fetch('runs')
  run = fixture['run'] ? runs.find { |r| r['id'] == fixture['run'] } : (runs.find { |r| r['status'] == 'completed' && r['nodes'].any? { |n| n['reused_from'] } } || runs.find { |r| r['status'] == 'completed' })
  raise 'existing fixture has no completed workflow for PTY verification' unless run && run['status'] == 'completed'
  version, err, status = Open3.capture3(binary, '--version')
  raise err unless status.success?
  turns_before = workflow_public_api(binary, env, 'turn.list', session_id: parent).map { |turn| turn['id'] }
  checks = []
  begin
    [[110, 34], [42, 22]].each do |columns, rows|
      %w[list get history].each do |operation|
        directory = File.join(output, "tui-#{columns}x#{rows}", operation); FileUtils.mkdir_p(directory)
        terminal = nil
        reader, writer, pid = PTY.spawn(env, binary, '--config', env.fetch('WILLDEEP_CONFIG'), '--profile', 'fixture', '--workspace', fixture.fetch('workspace'), '--resume', parent, '--language', 'en')
        begin
            terminal = WorkflowPty.new(reader, writer, directory, columns, rows)
            startup = terminal.wait_screen('startup') { |text| text.include?('Workflow') }
            raise 'startup already contains a stale command acknowledgement' if startup.include?('Workflow updated')
            command = operation == 'get' ? "/workflow get #{run.fetch('id')}" : "/workflow #{operation}"
            terminal.command(command, operation)
            expected = ['Workflow', 'Completed', run.dig('spec', 'name'), run.fetch('id')]
            if operation == 'get'
              expected += run['nodes'].map { |node| node['path'] }
              expected << 'Reused result' if run['nodes'].any? { |node| node['reused_from'] }
            end
            seen = []
            14.times do |index|
              text = terminal.snapshot("#{operation}-page-#{index}")
              raise 'workflow command failed in the real TUI' if text.include?('Workflow action failed')
              compact = text.gsub(/[\s│┃]/, '')
              seen |= expected.select { |word| compact.include?(word.delete(' ')) }
              break if (expected - seen).empty?
              terminal.input("\e[5~", 'PageUp')
            end
            raise "#{columns}-column #{operation} render missing: #{expected - seen}" unless (expected - seen).empty?
            terminal.input("\e[6~", 'PageDown'); terminal.snapshot('after-page-down')
            terminal.input("\x03", 'Ctrl-C')
            workflow_wait('Ctrl-C exits the real TUI', 5) do
              terminal.pump(0.05, allow_exit: true)
              Process.waitpid(pid, Process::WNOHANG)
            end
            raise 'TUI did not exit cleanly after Ctrl-C' unless $?.success?
            checks << { check: "real_pty_#{operation}_#{columns}x#{rows}", passed: true, columns: columns, rows: rows, rendered_details: seen, keyboard: ['Left', 'Delete', 'Enter', 'PageUp', 'PageDown', 'Ctrl-C'], evidence: directory }
        ensure
          begin
            terminal&.save
          ensure
            reader.close unless reader.closed?
            writer.close unless writer.closed?
            workflow_finish_child(pid)
          end
        end
      end
    end
    turns_after = workflow_public_api(binary, env, 'turn.list', session_id: parent).map { |turn| turn['id'] }
    raise 'read-only slash commands unexpectedly submitted a model turn' unless turns_after == turns_before
    report = { passed: true, version: version.strip, binary_sha256: Digest::SHA256.file(binary).hexdigest, checks: checks, parent: parent, run: run['id'], terminal_emulator: '@xterm/headless', physical_terminal_gui: false }
    write_workflow_report(output, report, 'tui-report')
    { check: 'real_pty_workflow_commands_wide_and_narrow', passed: true, report: File.join(output, 'tui-report.json'), checks: checks }
  rescue StandardError => error
    write_workflow_report(output, { passed: false, version: version.strip, checks: checks, error: "#{error.class}: #{error.message}" }, 'tui-report')
    raise
  end
end

if $PROGRAM_NAME == __FILE__
  options = { binary: ENV.fetch('WILLDEEP_BIN', File.expand_path('../target/debug/willdeep', __dir__)) }
  parser = OptionParser.new do |p|
    p.banner = 'Usage: ruby scripts/verify_workflow_tui.rb --fixture PATH [--binary PATH] [--output DIR]'
    p.on('--fixture PATH', 'Existing wd-workflow isolated fixture.json; does not stop its daemon/server') { |path| options[:fixture] = File.expand_path(path) }
    p.on('--binary PATH', 'Already-built binary') { |path| options[:binary] = File.expand_path(path) }
    p.on('--output DIR', 'JSON/Markdown and ANSI evidence directory') { |path| options[:output] = File.expand_path(path) }
    p.on('-h', '--help') { puts p; exit }
  end
  parser.parse!
  raise parser.to_s unless options[:fixture]
  fixture = JSON.parse(File.read(options[:fixture], encoding: Encoding::UTF_8))
  output = options.fetch(:output, File.dirname(options[:fixture]))
  puts JSON.pretty_generate(verify_workflow_tui(options[:binary], fixture, output))
end
