#!/usr/bin/env ruby
# frozen_string_literal: true

# Drive the real TUI and Runtime against a loopback model, in an isolated home.
require 'fileutils'
require 'io/console'
require 'json'
require 'pty'
require 'socket'
require 'tmpdir'

binary = File.expand_path(ARGV.fetch(0))
report_dir = File.expand_path(ARGV.fetch(1, 'docs/verification/0.154.0-rc11/slash-tui'))
raise 'willdeep binary is not executable' unless File.executable?(binary)
FileUtils.mkdir_p(report_dir)
root = Dir.mktmpdir('wd-unknown-slash-')
home = File.join(root, 'home')
workspace = File.join(root, 'workspace')
FileUtils.mkdir_p([home, workspace])
server = TCPServer.new('127.0.0.1', 0)
config = File.join(home, 'config.toml')
File.write(config, <<~CONFIG)
  version = 1
  default_provider = 'fixture'
  [agent]
  max_turns = 4
  approval = 'full-access'
  auto_title = false
  input_suggestions = false
  small_model_routing = false
  auto_dispatch_read_only = false
  safety_judge = false
  [local_model]
  enabled = false
  [providers.fixture]
  provider = 'openai-compatible'
  api = 'chat-completions'
  api_base = 'http://127.0.0.1:#{server.addr[1]}/v1'
  api_key = 'fixture'
  model = 'fixture'
CONFIG
File.chmod(0o600, config)
requests = []
errors = []
guard = Mutex.new
listener = Thread.new do
  loop do
    socket = server.accept
    begin
      method, = socket.gets.to_s.split(' ', 3)
      headers = {}
      while (line = socket.gets) && line != "\r\n"
        key, value = line.split(':', 2)
        headers[key.downcase] = value.strip if value
      end
      body = socket.read(headers.fetch('content-length', '0').to_i)
      if method == 'GET'
        socket.write("HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        next
      end
      request = JSON.parse(body)
      index = guard.synchronize { requests << request; requests.length }
      text = "SLASH_FIXTURE_REPLY_#{index}"
      if request['stream']
        socket.write("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n")
        [{ content: text }, {}].each_with_index do |delta, offset|
          event = { id: 'fixture', model: 'fixture', choices: [{ index: 0, delta: delta, finish_reason: offset.zero? ? nil : 'stop' }] }
          event[:usage] = { prompt_tokens: 20, completion_tokens: 5, total_tokens: 25 } unless offset.zero?
          socket.write("data: #{JSON.generate(event)}\n\n")
        end
        socket.write("data: [DONE]\n\n")
      else
        response = JSON.generate(choices: [{ message: { role: 'assistant', content: text }, finish_reason: 'stop' }], usage: { prompt_tokens: 20, completion_tokens: 5, total_tokens: 25 })
        socket.write("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: #{response.bytesize}\r\nConnection: close\r\n\r\n#{response}")
      end
    rescue StandardError => error
      guard.synchronize { errors << "#{error.class}: #{error.message}" }
    ensure
      socket.close
    end
  end
rescue IOError, Errno::EBADF
  # The owning thread closes the fixture listener after verification.
end

env = {
  'HOME' => home,
  'USERPROFILE' => home,
  'XDG_CONFIG_HOME' => File.join(home, 'config'),
  'WILLDEEP_HOME' => home,
  'WILLDEEP_LOCAL_DISCOVERY' => '0',
  'TERM' => 'xterm-256color',
  'WILLDEEP_TELEMETRY_DISABLED' => '1',
  'SSH_CONNECTION' => 'fixture'
}
%w[WILLDEEP_API_KEY WILLDEEP_API_BASE WILLDEEP_MODEL OPENAI_API_KEY OPENAI_BASE_URL ANTHROPIC_API_KEY ANTHROPIC_BASE_URL WILLDEEP_CONFIG].each { |key| env[key] = nil }
wait_for_exit = lambda do |pid, seconds|
  deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + seconds
  loop do
    break true if Process.waitpid(pid, Process::WNOHANG)
    break false if Process.clock_gettime(Process::CLOCK_MONOTONIC) >= deadline
    IO.select(nil, nil, nil, 0.05)
  end
rescue Errno::ECHILD
  true
end
runtime_stopped = false
stop_runtime = lambda do
  next if runtime_stopped
  pid = Process.spawn(env, binary, '--config', config, 'daemon', 'stop', out: File::NULL, err: File::NULL)
  unless wait_for_exit.call(pid, 5)
    Process.kill('KILL', pid)
    wait_for_exit.call(pid, 1)
    guard.synchronize { errors << 'fixture daemon stop exceeded 5 seconds' }
  end
  runtime_stopped = true
end
bytes = +''
cases = []
failure = nil
prompts = [
  '/tmp/slash-fixture/project Please inspect this directory',
  '/wat Please treat this as ordinary input',
  '/help/file Please inspect this file'
]
begin
  PTY.spawn(env, binary, '--config', config, '--workspace', workspace, '--language', 'en', '--new-session') do |reader, writer, pid|
    reader.winsize = [36, 140]
    drain = lambda do |wait|
      next unless IO.select([reader], nil, nil, wait)
      part = reader.read_nonblock(1_000_000, exception: false)
      raise 'TUI closed unexpectedly' if part.nil?
      next unless part.is_a?(String)
      bytes << part
      writer.write("\e[6;18;9t\e[0n") if part.include?("\e[16t") || part.include?("\e[5n")
    end
    wait_for = lambda do |name, &condition|
      deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + 25
      until condition.call
        raise "timed out: #{name}" if Process.clock_gettime(Process::CLOCK_MONOTONIC) > deadline
        drain.call(0.05)
      end
    end
    begin
      wait_for.call('TUI welcome') { bytes.include?('Hello,') }
      prompts.each_with_index do |prompt, index|
        writer.write("\e[200~#{prompt}\e[201~\r")
        wait_for.call("model receives prompt #{index + 1}") do
          guard.synchronize { requests.any? { |request| request.fetch('messages', []).any? { |message| message['role'] == 'user' && message['content'] == prompt } } }
        end
        wait_for.call("reply #{index + 1}") { bytes.include?("SLASH_FIXTURE_REPLY_#{index + 1}") }
        # Wait for the durable terminal event too, so the next case exercises idle submission.
        events = File.join(home, 'runtime', 'events.ndjson')
        wait_for.call("completed turn #{index + 1}") do
          File.exist?(events) && File.foreach(events).count { |line| JSON.parse(line)['kind'] == 'turn.completed' } >= index + 1
        end
        cases << { prompt: prompt, passed: true }
      end
      before = guard.synchronize { requests.length }
      offset = bytes.length
      writer.write("\e[200~/verbose on\e[201~\r")
      wait_for.call('known command usage error') { bytes[offset..].to_s.downcase.include?('usage:') }
      cases << { command: '/verbose on', local_error: true, passed: guard.synchronize { requests.length == before } }
      writer.write("\e[200~/exit\e[201~\r")
    ensure
      File.binwrite(File.join(report_dir, 'terminal.ansi'), bytes)
      # Close the PTY master before waiting: macOS can keep a terminating child
      # in tty drain while unread terminal output remains buffered.
      reader.close unless reader.closed?
      writer.close unless writer.closed?
      stop_runtime.call
      begin
        Process.kill('TERM', pid)
        unless wait_for_exit.call(pid, 3)
          Process.kill('KILL', pid)
          unless wait_for_exit.call(pid, 1)
            guard.synchronize { errors << 'fixture PTY did not exit within cleanup budget' }
          end
        end
      rescue Errno::ESRCH, Errno::ECHILD
        # The TUI may have already exited after /exit.
      end
    end
  end
rescue StandardError => error
  failure = "#{error.class}: #{error.message}"
ensure
  stop_runtime.call
  server.close
  unless listener.join(2)
    listener.kill
    listener.join(1)
    errors << 'fixture listener exceeded cleanup budget'
  end
  report = { passed: failure.nil? && errors.empty? && cases.length == 4 && cases.all? { |item| item[:passed] }, failure: failure, fixture_errors: errors, model_requests: requests.length, real_model_requests: 0, cases: cases }
  File.write(File.join(report_dir, 'report.json'), JSON.pretty_generate(report) + "\n")
  File.write(File.join(report_dir, 'report.md'), "# 未知斜杠输入真实 TUI 验收\n\n- 结果：#{report[:passed] ? '通过' : '失败'}\n- 隔离 TUI、Runtime 和本机假模型，真实模型请求 0 次。\n- 用例：#{cases.length}/4\n- 失败：#{failure || '无'}\n\n" + cases.map { |item| "- #{item[:passed] ? '通过' : '失败'}：`#{item[:prompt] || item[:command]}`" }.join("\n") + "\n")
  puts JSON.generate(report)
end
exit(failure.nil? && cases.length == 4 && cases.all? { |item| item[:passed] } && errors.empty? ? 0 : 1)
