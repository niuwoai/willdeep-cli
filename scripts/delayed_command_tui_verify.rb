#!/usr/bin/env ruby
# Real TUI + Runtime: no background flag, Enter during sleep, once-only completion.
require 'socket'
require 'json'
require 'tmpdir'
require 'fileutils'
require 'pty'
require 'io/console'
require 'timeout'

binary = File.expand_path(ARGV.fetch(0))
root = Dir.mktmpdir('wd-delayed-command-', '/private/tmp')
home = File.join(root, 'home')
workspace = File.join(root, 'workspace')
FileUtils.mkdir_p([home, File.join(workspace, 'nested')])
server = TCPServer.new('127.0.0.1', 0)
config = File.join(home, 'config.toml')
File.write(config, "version = 1\ndefault_provider = 'fixture'\n[agent]\nmax_turns = 4\napproval = 'full-access'\nauto_title = false\ninput_suggestions = false\nsmall_model_routing = false\nauto_dispatch_read_only = false\nsafety_judge = false\n[providers.fixture]\nprovider = 'openai-compatible'\napi = 'chat-completions'\napi_base = 'http://127.0.0.1:#{server.addr[1]}/v1'\napi_key = 'local-fixture-only'\nmodel = 'fixture'\n")
File.chmod(0600, config)
followup = 'operator-followup-during-delayed-command'
log = File.join(workspace, 'execution.log')
requests = []
guard = Mutex.new
followup_before_completion = false
command_count = 0
listener = Thread.new do
  loop do
    socket = server.accept
    Thread.new(socket) do |s|
      begin
        method, path = s.gets.split(' ', 3)
        headers = {}
        while (line = s.gets) && line != "\r\n"
          key, value = line.split(':', 2)
          headers[key.downcase] = value.strip if value
        end
        body = s.read(headers.fetch('content-length', '0').to_i)
        if method == 'GET'
          s.write("HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
          next
        end
        request = JSON.parse(body)
        index = guard.synchronize do
          n = requests.size
          requests << request
          if request.fetch('messages', []).any? { |m| m['role'] == 'user' && m['content'] == followup }
            followup_before_completion ||= !File.exist?(log)
          end
          n
        end
        if index.zero?
          command_count += 1
          arguments = {command: "sleep 30; cd nested && printf 'delayed-%s\\n' done >> ../execution.log && printf 'result-%s' ready", label: 'delayed command TUI verification', timeout_seconds: 60, run_in_background: false}
          delta = {tool_calls: [{index: 0, id: 'delayed-once', type: 'function', function: {name: 'run_command', arguments: arguments.to_json}}]}
          finish = 'tool_calls'
          message = {role: 'assistant', content: nil, tool_calls: delta[:tool_calls].map { |c| c.reject { |k, _| k == :index } }}
        else
          text = request.to_json.include?('result-ready') ? 'Background completion verified.' : 'Ready for more input while the background command continues.'
          delta = {content: text}
          finish = 'stop'
          message = {role: 'assistant', content: text}
        end
        if request['stream']
          s.write("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n")
          s.write("data: #{({id: 'fixture', model: 'fixture', choices: [{index: 0, delta: delta, finish_reason: nil}]}).to_json}\n\n")
          s.write("data: #{({id: 'fixture', model: 'fixture', choices: [{index: 0, delta: {}, finish_reason: finish}], usage: {prompt_tokens: 10, completion_tokens: 10, total_tokens: 20}}).to_json}\n\ndata: [DONE]\n\n")
        else
          response = {choices: [{message: message, finish_reason: finish}], usage: {prompt_tokens: 10, completion_tokens: 10, total_tokens: 20}}.to_json
          s.write("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: #{response.bytesize}\r\nConnection: close\r\n\r\n#{response}")
        end
      rescue Errno::EPIPE, Errno::ECONNRESET
      ensure
        s.close
      end
    end
  end
rescue IOError, Errno::EBADF
end
env = {'WILLDEEP_HOME' => home, 'TERM' => 'xterm-256color', 'WILLDEEP_TELEMETRY_DISABLED' => '1'}
%w[WILLDEEP_API_KEY WILLDEEP_API_BASE WILLDEEP_MODEL OPENAI_API_KEY OPENAI_BASE_URL ANTHROPIC_API_KEY ANTHROPIC_BASE_URL].each { |key| env[key] = nil }
bytes = ''.b
started = false
sent = false
completion = false
begin
  PTY.spawn(env, binary, '--config', config, '--workspace', workspace, '--language', 'en', '--new-session') do |reader, writer, pid|
    reader.winsize = [36, 130]
    began = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    begin
      loop do
        elapsed = Process.clock_gettime(Process::CLOCK_MONOTONIC) - began
        if IO.select([reader], nil, nil, 0.04)
          part = reader.read_nonblock(1_000_000, exception: false)
          break if part.nil?
          unless part == :wait_readable
            bytes << part
            writer.write("\e[6;18;9t\e[0n") if part.include?("\e[16t") || part.include?("\e[5n")
          end
        end
        if !started && bytes.include?('Hello,')
          writer.write("start the delayed check\r")
          started = true
        end
        events = File.join(home, 'runtime', 'events.ndjson')
        if !sent && File.exist?(events) && File.read(events).include?('background_wait')
          writer.write("#{followup}\r")
          sent = true
        end
        completion = guard.synchronize { requests.any? { |r| r.to_json.include?('result-ready') } }
        break if completion && followup_before_completion && File.exist?(log)
        raise 'delayed command TUI verification timed out' if elapsed > 50
      end
    rescue Errno::EIO
      raise 'TUI exited before verification completed'
    ensure
      File.binwrite(File.join(root, 'tui.ansi'), bytes)
      Process.kill('TERM', pid) rescue Errno::ESRCH
      begin
        Timeout.timeout(2) { Process.wait(pid) }
      rescue Timeout::Error
        Process.kill('KILL', pid) rescue Errno::ESRCH
      rescue Errno::ECHILD
      end
    end
  end
  saved = Dir[File.join(home, 'sessions', '*.json')].map { |path| JSON.parse(File.read(path)) }
  followup_count = saved.sum { |s| s.fetch('messages').count { |m| m['role'] == 'user' && m['content'] == followup } }
  visible_hint = bytes.include?('Delayed') && bytes.include?('Keep') && bytes.include?('typing')
  report = {root: root, enter_sent: sent, followup_read_before_command_completion: followup_before_completion, completion_delivered: completion, execution_count: File.read(log).lines.size, command_issued: command_count, followup_saved_count: followup_count, visible_hint: visible_hint}
  raise "verification failed: #{report}" unless sent && followup_before_completion && completion && File.read(log) == "delayed-done\n" && command_count == 1 && followup_count == 1 && visible_hint
  report[:passed] = true
  File.write(File.join(root, 'report.json'), JSON.pretty_generate(report))
  File.write(File.join(root, 'report.md'), "# Delayed command TUI verification\n\n" + report.map { |key, value| "- #{key}: #{value}" }.join("\n") + "\n")
  puts report.to_json
ensure
  system(env, binary, 'daemon', 'stop', out: File::NULL, err: File::NULL)
  server.close
  listener.join
end
