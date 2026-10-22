#!/usr/bin/env ruby
# Paced SSE -> real Runtime -> real TUI -> timestamped PTY -> terminal emulator.
# Uses only synthetic history, isolated WILLDEEP_HOME and a loopback provider.
# ruby scripts/stream_latency_tui_verify.rb BINARY REPORT_PREFIX [HISTORY_COUNT] [runtime|local]
# STREAM_UI_MODE=full selects the full layout; STREAM_MAX_P95_MS sets an acceptance gate.
require 'base64'
require 'fileutils'
require 'io/console'
require 'json'
require 'open3'
require 'pty'
require 'securerandom'
require 'socket'
require 'timeout'
require 'tmpdir'

binary = File.expand_path(ARGV.fetch(0))
report_prefix = File.expand_path(ARGV.fetch(1))
history_count = Integer(ARGV.fetch(2, '400'))
journal_count = Integer(ENV.fetch('STREAM_JOURNAL_EVENTS', '0'))
execution = ARGV.fetch(3, 'runtime')
raise 'execution must be runtime or local' unless %w[runtime local].include?(execution)
count = Integer(ENV.fetch('STREAM_EVENTS', '160'))
interval = Float(ENV.fetch('STREAM_INTERVAL', '0.02'))
ui_mode = ENV.fetch('STREAM_UI_MODE', 'compact')
raise 'STREAM_UI_MODE must be compact or full' unless %w[compact full].include?(ui_mode)
root = Dir.mktmpdir('wd-stream-', '/private/tmp')
home, workspace = %w[home workspace].map { |name| File.join(root, name) }
FileUtils.mkdir_p([File.join(home, 'sessions'), workspace, File.dirname(report_prefix)])
if journal_count > 0
  FileUtils.mkdir_p(File.join(home, 'runtime'))
  File.open(File.join(home, 'runtime/events.ndjson'), 'w', 0600) do |file|
    journal_count.times do |index|
      file.puts(JSON.generate(sequence: index + 1, timestamp: 1, kind: 'fixture.historical', message: 'pre-existing unrelated Runtime event'))
    end
  end
end
session_id = SecureRandom.uuid
session_path = File.join(home, 'sessions', "#{session_id}.json")
history = Array.new(history_count) do |index|
  {role: index.even? ? 'user' : 'assistant', content: "History #{index}: " + '已完成的历史记录 rendering history with **bold** and `code`. ' * 8}
end
File.write(session_path, JSON.generate({version: 1, id: session_id, title: 'Stream latency fixture',
  workspace: workspace, profile: 'fixture', created_at: 1, updated_at: 1,
  messages: history, runtime_event_cursor: 0}))
server = TCPServer.new('127.0.0.1', 0)
sent, requests, frames, workers, connections, write_times = [], [], [], [], [], []
runtime_seen = {}
observer = observer_socket = runtime_stream_status = nil
clock = -> { Process.clock_gettime(Process::CLOCK_MONOTONIC) }
listener = Thread.new do
  loop do
    client = server.accept
    connections << client
    workers << Thread.new(client) do |socket|
      begin
        socket.setsockopt(Socket::IPPROTO_TCP, Socket::TCP_NODELAY, 1)
        method, path = socket.gets.to_s.split
        headers = {}
        while (line = socket.gets) && line != "\r\n"
          key, value = line.split(':', 2)
          headers[key.downcase] = value.strip if value
        end
        body = socket.read(headers.fetch('content-length', '0').to_i)
        if method != 'POST'
          socket.write("HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
          next
        end
        request = JSON.parse(body)
        requests << {path: path, model: request['model'], stream: request['stream']}
        socket.write("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n")
        # Let task-start events reach the TUI before the paced answer begins.
        sleep 0.4
        started = clock.call
        count.times do |index|
          sleep [started + index * interval - clock.call, 0].max
          marker = format('FLOW%04d', index)
          content = "#{marker} 新字中文\n"
          sent << {marker: marker, at: clock.call, content: content}
          socket.write("data: #{JSON.generate({choices: [{index: 0, delta: {content: content}}]})}\n\n")
        end
        socket.write("data: #{JSON.generate({choices: [{index: 0, delta: {}, finish_reason: 'stop'}]})}\n\ndata: [DONE]\n\n")
      rescue IOError, Errno::EPIPE, Errno::ECONNRESET
      ensure
        socket.close unless socket.closed?
      end
    end
  end
rescue IOError, Errno::EBADF
end
config = File.join(home, 'config.toml')
File.write(config, [
  'version = 1', "default_provider = 'fixture'", '[agent]', 'max_turns = 2',
  'auto_title = false', 'input_suggestions = false', 'small_model_routing = false',
  'auto_dispatch_read_only = false', '[providers.fixture]', "provider = 'openai-compatible'",
  "api = 'chat-completions'", "api_base = 'http://127.0.0.1:#{server.addr[1]}/v1'",
  "api_key = 'fixture-only'", "model = 'fixture'", 'context_window = 2000000',
  '[ui]', "mode = '#{ui_mode}'", 'check_updates = false', ''
].join("\n"), perm: 0600)
env = {'WILLDEEP_HOME' => home, 'TERM' => 'xterm-256color', 'COLORTERM' => 'truecolor', 'WILLDEEP_LOCAL_DISCOVERY' => '0',
  'WILLDEEP_TELEMETRY_DISABLED' => '1', 'NO_PROXY' => '127.0.0.1,localhost', 'no_proxy' => '127.0.0.1,localhost'}
%w[WILLDEEP_API_BASE WILLDEEP_API_KEY WILLDEEP_MODEL WILLDEEP_CONFIG OPENAI_BASE_URL OPENAI_API_KEY ANTHROPIC_BASE_URL ANTHROPIC_API_KEY].each { |key| env[key] = nil }
bytes = ''.b
graceful_exit = false
begin
  PTY.spawn(env, binary, '--config', config, '--workspace', workspace, '--language', 'en', '--resume', session_id) do |reader, writer, pid|
    reader.winsize = [35, 120]
    started = clock.call
    submitted = false
    completed_at = nil
    painted_completion_at = nil
    last_mtime = nil
    sampled = false
    begin
      loop do
        if IO.select([reader], nil, nil, 0.01)
          part = reader.read_nonblock(1_000_000, exception: false)
          raise 'TUI exited before completion' if part.nil?
          if part.is_a?(String)
            frames << {at: clock.call, data: Base64.strict_encode64(part)}
            bytes << part
            writer.write("\e[6;18;9t\e[0n") if part.include?("\e[16t") || part.include?("\e[5n")
          end
        end
        if !submitted && bytes.include?('Enter') && clock.call - started > 1
          writer.write("#{execution == 'local' ? '/local ' : ''}Produce the paced stream fixture without tools\r")
          submitted = true
        end
        if submitted && execution == 'runtime' && observer.nil? && File.file?(File.join(home, 'runtime', 'daemon.json'))
          state = JSON.parse(File.read(File.join(home, 'runtime', 'daemon.json')))
          host, port = state.fetch('address').split(':')
          observer_socket = TCPSocket.new(host, Integer(port))
          observer_socket.write("GET /v1/events/stream.ndjson?after=#{journal_count}&limit=1000 HTTP/1.1\r\nHost: localhost\r\nx-willdeep-token: #{state.fetch('token')}\r\nConnection: close\r\n\r\n")
          runtime_stream_status = observer_socket.gets.split[1].to_i
          raise "Runtime stream HTTP #{runtime_stream_status}" unless runtime_stream_status == 200
          while (header_line = observer_socket.gets) && header_line != "\r\n"
            # Discard headers; the observer timestamps NDJSON events only.
          end
          observer = Thread.new do
            while (line = observer_socket.gets)
              next unless line.start_with?('{')
              data = JSON.parse(line).fetch('data', {})
              payload = data.fetch('message', '').split(' ', 2)[1]
              next unless payload&.start_with?('{')
              value = JSON.parse(payload)
              next unless value['type'] == 'assistant_text_delta'
              value.fetch('text', '').scan(/FLOW\d{4}/).each { |marker| runtime_seen[marker] ||= clock.call }
            end
          rescue IOError, Errno::ECONNRESET
          end
        end
        mtime = File.mtime(session_path).to_r
        if last_mtime && mtime != last_mtime
          write_times << clock.call
        end
        last_mtime = mtime
        if ENV['STREAM_SAMPLE'] && !sampled && sent.size >= 20
          sampled = true
          workers << Thread.new { Open3.capture3('sample', pid.to_s, '2', '1', '-file', File.join(root, 'tui.sample.txt')) }
        end
        if submitted && sent.size == count && clock.call - sent.last[:at] > 0.5
          saved = JSON.parse(File.read(session_path)) rescue {}
          if saved.fetch('messages', []).any? { |m| m['role'] == 'assistant' && m['content'].strip == sent.map { |s| s[:content] }.join.strip }
            completed_at ||= clock.call
          end
        end
        painted_completion_at ||= clock.call if bytes.include?('turn finished')
        break if completed_at && painted_completion_at && clock.call - [completed_at, painted_completion_at].max > 1
        raise "stream timeout: requests=#{requests.size}, sent=#{sent.size}, root=#{root}" if clock.call - started > 100
      end
      writer.write("\x03")
      Timeout.timeout(10) do
        loop do
          part = reader.readpartial(1_000_000)
          frames << {at: clock.call, data: Base64.strict_encode64(part)}
          bytes << part
        end
      rescue Errno::EIO, EOFError
        graceful_exit = true
      end
    ensure
      Process.kill('TERM', pid) rescue Errno::ESRCH
      begin
        Timeout.timeout(3) { Process.wait(pid) }
      rescue Timeout::Error
        Process.kill('KILL', pid) rescue Errno::ESRCH
      rescue Errno::ECHILD
      end
    end
  end
  File.binwrite(File.join(root, 'terminal.ansi'), bytes)
  input_path = File.join(root, 'frames.json')
  File.write(input_path, JSON.generate({frames: frames, sent: sent}))
  output, error, status = Open3.capture3('node', File.join(__dir__, 'stream_latency_terminal.mjs'), input_path)
  raise "terminal reconstruction failed: #{error}" unless status.success?
  measured = JSON.parse(output)
  raise "expected one inference request, got #{requests.size}" unless requests.size == 1
  report = measured.merge('binary' => binary, 'root' => root, 'history_entries' => history_count, 'historical_runtime_events' => journal_count,
    'execution' => execution, 'ui_mode' => ui_mode, 'requests' => requests, 'input_interval_ms' => interval * 1000,
    'runtime_stream_http_status' => runtime_stream_status,
    'observed_session_rewrites' => write_times.count { |t| t >= sent.first[:at] && t <= sent.last[:at] },
    'graceful_exit' => graceful_exit,
    'saved_answer_complete' => true)
  if execution == 'runtime'
    latest_sequence = JSON.parse(File.readlines(File.join(home, 'runtime', 'events.ndjson')).last).fetch('sequence')
    persisted_cursor = JSON.parse(File.read(session_path)).fetch('runtime_event_cursor')
    report['cursor_flushed_on_exit'] = latest_sequence == persisted_cursor
    raise 'graceful exit did not flush Runtime cursor' unless report['cursor_flushed_on_exit']
  end
  runtime_delays = sent.filter_map { |s| (runtime_seen[s[:marker]] - s[:at]) * 1000 if runtime_seen[s[:marker]] }.sort
  unless runtime_delays.empty?
    report['runtime_event_median_ms'] = runtime_delays[runtime_delays.size / 2]
    report['runtime_event_p95_ms'] = runtime_delays[(runtime_delays.size * 0.95).floor]
  end
  if ENV['STREAM_MAX_P95_MS']
    report['passed'] = report['displayed'] == count && report['p95_ms'] <= Float(ENV['STREAM_MAX_P95_MS'])
    report['acceptance_p95_ms'] = Float(ENV['STREAM_MAX_P95_MS'])
  end
  File.write(report_prefix + '.json', JSON.pretty_generate(report))
  File.write(report_prefix + '.md', "# Paced stream display measurement\n\n" + report.reject { |k, _| k == 'samples' }.map { |k, v| "- #{k}: #{v}" }.join("\n") + "\n")
  puts JSON.generate(report.reject { |k, _| k == 'samples' })
  raise 'stream display acceptance failed; see saved measurement report' if report['passed'] == false
ensure
  File.binwrite(File.join(root, 'terminal.ansi'), bytes)
  File.write(File.join(root, 'frames.json'), JSON.generate({frames: frames, sent: sent}))
  observer_socket&.close
  observer&.join(1)
  observer&.kill if observer&.alive?
  Open3.capture3(env, binary, 'daemon', 'stop')
  server.close
  listener.join(1)
  listener.kill if listener.alive?
  connections.each { |c| c.close unless c.closed? }
  workers.each { |w| w.join(1); w.kill if w.alive? }
end
