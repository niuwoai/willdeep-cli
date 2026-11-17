#!/usr/bin/env ruby
# frozen_string_literal: true

require 'digest'
require 'fileutils'
require 'io/console'
require 'json'
require 'open3'
require 'pty'
require 'socket'
require 'tmpdir'
require 'timeout'

binary = File.realpath(ARGV.fetch(0))
output = File.expand_path(ARGV.fetch(1))
FileUtils.mkdir_p(output)
root = Dir.mktmpdir('wd-custom-subagent-')
home, workspace = %w[home workspace].map { |name| File.join(root, name) }
FileUtils.mkdir_p([home, workspace], mode: 0o700)
checks, observed, errors = [], [], Queue.new
report = { passed: false, checks: checks, binary: binary, sha256: Digest::SHA256.file(binary).hexdigest }
report_path = File.join(output, 'custom-subagent-startup.json')
if File.file?(report_path)
  previous = JSON.parse(File.read(report_path))
  report[:previous_attempts] = previous.fetch('previous_attempts', []) + [previous.reject { |key, _| key == 'previous_attempts' }]
end
server = TCPServer.new('127.0.0.1', 0)
workers = []
listener = Thread.new do
  loop do
    socket = server.accept
    workers << Thread.new(socket) do |client|
      begin
        method, path = client.gets.to_s.split
        headers = {}
        while (line = client.gets) && line != "\r\n"
          key, value = line.split(':', 2)
          headers[key.downcase] = value.strip
        end
        body = client.read(Integer(headers.fetch('content-length', '0')))
        if method == 'GET'
          payload = JSON.generate({ data: [{ id: 'parent-fixture' }, { id: 'specialist-fixture' }] })
          client.write("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: #{payload.bytesize}\r\nConnection: close\r\n\r\n#{payload}")
          next
        end
        raise "unexpected fixture request: #{method} #{path}" unless path == '/v1/chat/completions'
        request = JSON.parse(body)
        messages = request.fetch('messages')
        called = messages.filter_map { |item| item['tool_calls'] }.flatten.map { |item| item.dig('function', 'name') }
        child = request.fetch('model') == 'specialist-fixture'
        if child
          raise 'specialist instructions missing' unless JSON.generate(messages).include?('CUSTOM_DOCUMENT_INSTRUCTIONS')
          observed << { model: request['model'], instructions_observed: true }
          message = { role: 'assistant', content: 'CUSTOM_DOCUMENT_RESULT' }
        elsif called.include?('spawn_agent')
          raise 'parent did not receive worker result' unless JSON.generate(messages).include?('CUSTOM_DOCUMENT_RESULT')
          message = { role: 'assistant', content: 'PARENT_RECEIVED_CUSTOM_DOCUMENT_RESULT' }
        else
          message = { role: 'assistant', content: nil, tool_calls: [{ id: 'custom-doc-call', type: 'function',
            function: { name: 'spawn_agent', arguments: JSON.generate({ profile: 'user/doc-multilingual',
              prompt: 'Return the bounded document result.', run_in_background: false }) } }] }
        end
        reason = message[:tool_calls] ? 'tool_calls' : 'stop'
        usage = { prompt_tokens: 100, completion_tokens: 10, total_tokens: 110 }
        if request['stream']
          delta = message.dup
          delta[:tool_calls] = delta[:tool_calls].map.with_index { |item, index| item.merge(index: index) } if delta[:tool_calls]
          client.write("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n")
          client.write("data: #{JSON.generate({ id: 'custom-agent-fixture', choices: [{ index: 0, delta: delta, finish_reason: nil }] })}\n\n")
          client.write("data: #{JSON.generate({ choices: [{ index: 0, delta: {}, finish_reason: reason }], usage: usage })}\n\ndata: [DONE]\n\n")
        else
          payload = JSON.generate({ id: 'custom-agent-fixture', object: 'chat.completion', model: request['model'],
            choices: [{ index: 0, message: message, finish_reason: reason }], usage: usage })
          client.write("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: #{payload.bytesize}\r\nConnection: close\r\n\r\n#{payload}")
        end
      rescue IOError, Errno::EPIPE, Errno::ECONNRESET
        # Owned CLI clients may close a capability probe or cancel at shutdown.
      rescue StandardError => error
        errors << "#{error.class}: #{error.message}"
      ensure
        client.close unless client.closed?
      end
    end
  end
rescue IOError, Errno::EBADF
  # The owned listener is closed in cleanup.
end
config = File.join(home, 'config.toml')
base = <<~TOML
  version=1
  default_provider='fixture'
  [agent]
  language='en'
  approval='full-access'
  safety_judge=false
  auto_title=false
  input_suggestions=false
  small_model_routing=false
  auto_dispatch_read_only=false
  max_turns=6
  [local_model]
  enabled=false
  [telemetry]
  enabled=false
  [ui]
  check_updates=false
  [providers.fixture]
  provider='openai-compatible'
  api='chat-completions'
  api_base='http://127.0.0.1:#{server.addr[1]}/v1'
  api_key='loopback-fixture'
  model='parent-fixture'
TOML
File.write(config, base + "\n[subagents.doc-multilingual]\ncontext_window=65536\n", perm: 0o600)
environment = { 'PATH' => ENV.fetch('PATH'), 'HOME' => home, 'WILLDEEP_HOME' => home,
  'WILLDEEP_LOCAL_DISCOVERY' => '0', 'WILLDEEP_TELEMETRY_DISABLED' => '1',
  'WILLDEEP_WORKSPACE_CHECKPOINTS' => '0', 'TERM' => 'xterm-256color',
  'SSH_CONNECTION' => '127.0.0.1 1 127.0.0.1 2', 'LANG' => 'en_US.UTF-8',
  'NO_PROXY' => '127.0.0.1,localhost', 'no_proxy' => '127.0.0.1,localhost' }
run = lambda do |*args, success: true|
  stdout, stderr, status = Timeout.timeout(60) do
    Open3.capture3(environment, binary, '--config', config, '--workspace', workspace, *args,
      unsetenv_others: true, chdir: workspace)
  end
  raise "#{args.first}: #{stderr}" if success && !status.success?
  [stdout, stderr, status]
end
begin
  report[:version] = run.call('--version').first.strip
  _, error, status = run.call('config', 'check', success: false)
  raise 'misplaced custom section was accepted' if status.success?
  [config, '[subagents.doc-multilingual]', 'willdeep agents bind'].each do |expected|
    raise "missing repair diagnostic: #{expected}" unless error.include?(expected)
  end
  checks << 'unknown section fails with config path and actionable library guidance'
  File.write(config, base, perm: 0o600)
  run.call('config', 'check')
  checks << 'valid config loads after preserving/removing the misplaced section'
  instructions = File.join(root, 'instructions.md')
  File.write(instructions, "CUSTOM_DOCUMENT_INSTRUCTIONS: preserve every source section.\n", perm: 0o600)
  run.call('agents', 'create', 'user/doc-multilingual', 'Document specialist', 'Translate documents',
    '--instructions-file', instructions, '--tool', 'workspace.read')
  run.call('agents', 'bind', 'user/doc-multilingual', '--provider-profile', 'fixture',
    '--model', 'specialist-fixture', '--context-window', '65536')
  items = JSON.parse(run.call('agents', 'list').first).fetch('items')
  item = items.find { |entry| entry.dig('installation', 'definition_id') == 'user/doc-multilingual' }
  raise 'custom agent missing or disabled' unless item&.dig('installation', 'enabled')
  binding = item.fetch('installation').fetch('binding')
  raise 'local binding differs' unless binding['model'] == 'specialist-fixture' && binding['context_window'] == 65_536
  checks << 'custom definition remains enabled with its independent model and 65536 window binding'
  terminal = ''.b
  PTY.spawn(environment, binary, '--config', config, '--workspace', workspace, '--language', 'en',
    '--new-session', unsetenv_others: true, chdir: workspace) do |reader, writer, pid|
    reader.winsize = [36, 120]
    wait_for = lambda do |marker|
      deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + 30
      until terminal.include?(marker)
        raise "TUI timeout waiting for #{marker}" if Process.clock_gettime(Process::CLOCK_MONOTONIC) > deadline
        next unless IO.select([reader], nil, nil, 0.05)
        part = reader.read_nonblock(1_000_000, exception: false)
        raise 'TUI exited before completing the interaction' if part.nil?
        next unless part.is_a?(String)
        terminal << part
        writer.write("\e[6;18;9t\e[0n") if part.include?("\e[16t") || part.include?("\e[5n")
      end
    end
    begin
      wait_for.call('Hello,')
      checks << 'real PTY reaches the interactive greeting with the custom agent installed'
      writer.write("/agents\r")
      writer.flush
      # The JSON list scrolls its ID above the viewport; assert the visible
      # instruction marker from the installed definition, not an offscreen row.
      wait_for.call('CUSTOM_DOCUMENT_INSTRUCTIONS')
      checks << 'TUI /agents interaction returns the installed custom definition'
    ensure
      File.binwrite(File.join(output, 'custom-subagent-startup.ansi'), terminal)
      # Close both PTY handles before waiting: unread terminal output can
      # otherwise leave a macOS child stuck exiting, even after SIGKILL.
      writer.close unless writer.closed?
      reader.close unless reader.closed?
      begin
        reaped = false
        %w[TERM KILL].each do |signal|
          Process.kill(signal, pid) rescue Errno::ESRCH
          deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + 5
          loop do
            reaped = !!Process.waitpid(pid, Process::WNOHANG)
            break if reaped || Process.clock_gettime(Process::CLOCK_MONOTONIC) >= deadline
            IO.select(nil, nil, nil, 0.05)
          end
          break if reaped
        end
        raise 'owned PTY child could not be reaped' unless reaped
      rescue Errno::ECHILD
        # The owned PTY child was already reaped.
      end
    end
  end
  %w[local runtime].each do |mode|
    args = ['run']
    args << '--local' if mode == 'local'
    args.concat(['--output', 'json', 'Delegate the bounded document task to user/doc-multilingual.'])
    stdout, = run.call(*args)
    raise "#{mode} did not return the custom result" unless stdout.include?('PARENT_RECEIVED_CUSTOM_DOCUMENT_RESULT')
    checks << "#{mode} CLI dispatch executes custom instructions and returns its result to the parent"
  end
  raise 'custom model never executed' unless observed.size == 2
  raise errors.pop unless errors.empty?
  checks << 'both dispatch paths used the specialist model without contacting external providers'
  report[:observed] = observed
  report[:passed] = true
rescue StandardError => error
  report[:error] = "#{error.class}: #{error.message}"
ensure
  begin
    run.call('daemon', 'stop')
  rescue StandardError => error
    report[:cleanup_error] = "#{error.class}: #{error.message}"
    report[:passed] = false
  end
  server.close
  listener.join(2)
  workers.each { |worker| worker.join(2) }
  FileUtils.remove_entry(root) if report[:passed]
  report[:fixture_preserved] = File.directory?(root)
  report[:fixture] = root if report[:fixture_preserved]
  File.write(File.join(output, 'custom-subagent-startup.json'), JSON.pretty_generate(report) + "\n")
  lines = checks.map { |check| "- PASS #{check}" }
  text = "# 自定义智能体启动回归\n\n版本：#{report[:version]}。结果：#{report[:passed] ? '通过' : '失败'}。临时配置、临时智能体库与 Git 工作区外的临时目录，仅使用回环模型夹具。\n\n#{lines.join("\n")}\n\n#{report[:error]}\n#{report[:cleanup_error]}\n"
  File.write(File.join(output, 'custom-subagent-startup.md'), text)
end
puts JSON.pretty_generate(report)
exit(report[:passed] ? 0 : 1)
