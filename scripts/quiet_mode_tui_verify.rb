require 'json'
require 'socket'
require 'tmpdir'
require 'fileutils'
require 'open3'
require 'pty'
require 'io/console'
require 'timeout'

binary = File.expand_path(ARGV.fetch(0))
root = Dir.mktmpdir('wdq-', '/tmp')
home = File.join(root, 'home')
workspace = File.join(root, 'ws')
FileUtils.mkdir_p([home, workspace])
server = TCPServer.new('127.0.0.1', 0)
requests = []
connections = []
workers = []
listener = Thread.new do
  loop do
    client = server.accept
    connections << client
    workers << Thread.new(client) do |client|
      begin
        method = client.gets.to_s.split.first
        headers = {}
        while (line = client.gets) && line != "\r\n"
          key, value = line.split(':', 2)
          headers[key.downcase] = value.strip if value
        end
        body = client.read(headers.fetch('content-length', '0').to_i)
        if method != 'POST'
          client.write("HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        else
          requests << JSON.parse(body)
          sleep 20
          payload = {choices: [{index: 0, delta: {content: 'QUIET_FIXTURE_DONE'}}]}
          client.write("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: #{JSON.generate(payload)}\n\n")
          stop = {choices: [{index: 0, delta: {}, finish_reason: 'stop'}]}
          client.write("data: #{JSON.generate(stop)}\n\ndata: [DONE]\n\n")
        end
      rescue IOError, Errno::EPIPE, Errno::ECONNRESET
        # A client may close an idle connection when the fixture shuts down.
      ensure
        client.close unless client.closed?
      end
    end
  end
rescue IOError, Errno::EBADF
end
env = {'WILLDEEP_HOME' => home, 'TERM' => 'xterm-256color', 'COLORTERM' => 'truecolor', 'NO_COLOR' => nil, 'CLICOLOR_FORCE' => '1', 'WILLDEEP_TELEMETRY_DISABLED' => '1'}
env['NO_PROXY'] = env['no_proxy'] = '127.0.0.1,localhost'
%w[WILLDEEP_API_BASE WILLDEEP_API_KEY WILLDEEP_MODEL WILLDEEP_CONFIG OPENAI_BASE_URL OPENAI_API_KEY ANTHROPIC_BASE_URL ANTHROPIC_API_KEY].each { |key| env[key] = nil }
config = File.join(home, 'config.toml')
File.write(config, "version = 1\ndefault_provider = 'fixture'\n[agent]\nmax_turns = 2\nauto_title = false\ninput_suggestions = false\nsmall_model_routing = false\nauto_dispatch_read_only = false\n[providers.fixture]\nprovider = 'openai-compatible'\napi = 'chat-completions'\napi_base = 'http://127.0.0.1:#{server.addr[1]}/v1'\napi_key = 'fixture-only'\nmodel = 'fixture'\n", perm: 0600)
bytes = ''.b
phases = []
begin
  PTY.spawn(env, binary, '--config', config, '--workspace', workspace, '--language', 'en', '--new-session') do |reader, writer, pid|
    reader.winsize = [35, 120]
    pump = lambda do |seconds|
      deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + seconds
      while Process.clock_gettime(Process::CLOCK_MONOTONIC) < deadline
        if IO.select([reader], nil, nil, 0.03)
          part = reader.read_nonblock(1_000_000, exception: false)
          raise 'TUI exited unexpectedly' if part.nil?
          if part.is_a?(String)
            bytes << part
            writer.write("\e[6;18;9t\e[0n") if part.include?("\e[16t") || part.include?("\e[5n")
          end
        end
      end
    end
    snapshot = lambda do |name, compact|
      deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + 12
      data = nil
      loop do
        path = File.join(root, "#{name}.ansi")
        File.binwrite(path, bytes)
        out, error, status = Open3.capture3('node', File.join(__dir__, 'terminal_snapshot.mjs'), path, '', '120', '35')
        raise error unless status.success?
        data = JSON.parse(out)
        File.write(File.join(root, "#{name}.json"), JSON.pretty_generate(data))
        problem = if compact && data['text'].match?(/┌|└|│|Activity|Workspace write|Your turn/)
          'chrome remained'
        elsif compact && data['input_bg'] == data['chat_bg']
          'input background indistinguishable'
        elsif !compact && !data['text'].include?('┌')
          'detail did not return'
        elsif name == 'first-key-wakes'
          row = data['lines'].index { |line| line.include?('Prompt [focused]') }
          'first typed character was lost' unless row && data['lines'][row + 1].include?('a')
        elsif name == 'draft-retained-quiet'
          'draft vanished' unless data['lines'][32].strip == 'a'
        elsif name == 'sidebar-pins-detailed'
          'sidebar not displayed' unless data['text'].include?('Status')
        elsif name == 'queue-pins-detailed'
          'queue window missing' unless data['text'].match?(/queue/i)
        end
        break unless problem
        raise "#{name}: #{problem}" if Process.clock_gettime(Process::CLOCK_MONOTONIC) >= deadline
        pump.call(0.2)
      end
      phases << {name: name, compact: compact, passed: true}
      data
    end
    begin
      startup_deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + 30
      until bytes.include?('Hello,')
        raise 'TUI startup timed out' if Process.clock_gettime(Process::CLOCK_MONOTONIC) >= startup_deadline
        pump.call(0.1)
      end
      writer.write("\e")
      pump.call(0.5)
      snapshot.call('initial-detailed', false)
      writer.write("Reply QUIET_FIXTURE_DONE without tools\r")
      request_deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + 60
      until requests.size == 1
        raise 'model request not received' if Process.clock_gettime(Process::CLOCK_MONOTONIC) >= request_deadline
        pump.call(0.1)
      end
      pump.call(6)
      snapshot.call('running-quiet', true)
      writer.write('a')
      pump.call(0.5)
      data = snapshot.call('first-key-wakes', false)
      row = data['lines'].index { |line| line.include?('Prompt [focused]') }
      raise 'first typed character was lost' unless row && data['lines'][row + 1].include?('a')
      pump.call(5.5)
      data = snapshot.call('draft-retained-quiet', true)
      raise 'draft vanished' unless data['lines'][32].strip == 'a'
      writer.write("\e[<0;3;33M\e[<0;3;33m")
      pump.call(0.5)
      snapshot.call('mouse-wakes', false)
      writer.write("\x02")
      pump.call(6)
      data = snapshot.call('sidebar-pins-detailed', false)
      raise 'sidebar not displayed' unless data['text'].include?('Status')
      writer.write("\x02")
      pump.call(5.5)
      snapshot.call('sidebar-closed-quiet', true)
      writer.write("\x11")
      pump.call(6)
      data = snapshot.call('queue-pins-detailed', false)
      raise 'queue window missing' unless data['text'].match?(/queue/i)
      writer.write("\e")
      pump.call(0.5)
      writer.write("\x7f/exit\r")
      sleep 0.2
    ensure
      File.binwrite(File.join(root, 'terminal.ansi'), bytes)
      Process.kill('TERM', pid) rescue Errno::ESRCH
      begin
        Timeout.timeout(2) { Process.wait(pid) }
      rescue Timeout::Error
        Process.kill('KILL', pid) rescue Errno::ESRCH
      rescue Errno::ECHILD
      end
    end
  end
  tasks = JSON.parse(File.read(File.join(home, 'runtime/tasks.json')))
  sessions = Dir[File.join(home, 'sessions/*.json')].map { |path| JSON.parse(File.read(path)) }
  answer_saved = sessions.any? do |session|
    session.fetch('messages').any? { |message| message['role'] == 'assistant' && message['content'] == 'QUIET_FIXTURE_DONE' }
  end
  raise 'task was interrupted or submitted twice' unless requests.size == 1 && tasks.size == 1 && tasks.first['status'] == 'completed' && answer_saved
  report = {passed: true, binary: binary, root: root, phases: phases, inference_requests: requests.size, first_character_preserved: true, task_completed: true}
  output = ARGV[1] || File.join(root, 'report')
  File.write(output + '.json', JSON.pretty_generate(report))
  File.write(output + '.md', "# Quiet mode PTY verification\n\n" + phases.map { |phase| "- #{phase[:name]}: PASS" }.join("\n") + "\n\nFirst typed character preserved; one inference completed.\n")
  puts report.to_json
ensure
  Open3.capture3(env, binary, 'daemon', 'stop')
  server.close
  listener.join(2)
  listener.kill if listener.alive?
  connections.each { |client| client.close unless client.closed? }
  workers.each { |worker| worker.join(1); worker.kill if worker.alive? }
end
