#!/usr/bin/env ruby
# Real PTY startup checks, using isolated sessions and a loopback-only Provider.
require 'pty'
require 'io/console'
require 'tmpdir'
require 'fileutils'
require 'json'
require 'securerandom'
require 'socket'
require 'timeout'

binary = File.expand_path(ARGV.fetch(0))
root = Dir.mktmpdir('wd-session-startup-', '/private/tmp')
home = File.join(root, 'home')
project = File.join(root, 'project')
other = File.join(root, 'other')
fresh = File.join(root, 'fresh')
FileUtils.mkdir_p([File.join(home, 'sessions'), project, other, fresh])
server = TCPServer.new('127.0.0.1', 0)
requests = []
listener = Thread.new do
  loop do
    socket = server.accept
    requests << socket.gets
    socket.write("HTTP/1.1 500 Fixture rejected unexpected model request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
    socket.close
  end
rescue IOError, Errno::EBADF
end
config = File.join(home, 'config.toml')
File.write(config, "version = 1\ndefault_provider = 'fixture'\n[agent]\nauto_title = false\ninput_suggestions = false\nsmall_model_routing = false\nauto_dispatch_read_only = false\n[providers.fixture]\nprovider = 'openai-compatible'\napi_base = 'http://127.0.0.1:#{server.addr[1]}/v1'\napi_key = 'local-fixture-only'\nmodel = 'fixture'\n")
File.chmod(0600, config)
ids = []
[[project, 'Project A', 'STARTUP_HISTORY_A', 10], [other, 'Project B', 'STARTUP_HISTORY_B', 20], [project, 'Empty session', nil, 30]].each do |workspace, title, text, updated|
  id = SecureRandom.uuid
  ids << id
  session = {version: 1, id: id, title: title, workspace: workspace, profile: 'fixture', created_at: 1, updated_at: updated, runtime_event_cursor: 1, messages: text ? [{role: 'user', content: text}, {role: 'assistant', content: 'Saved response'}] : []}
  File.write(File.join(home, 'sessions', "#{id}.json"), JSON.generate(session))
end
before = Dir[File.join(home, 'sessions', '*.json')].to_h { |p| [p, JSON.parse(File.read(p))['messages']] }
env = {'WILLDEEP_HOME' => home, 'TERM' => 'xterm-256color', 'WILLDEEP_TELEMETRY_DISABLED' => '1'}
%w[WILLDEEP_API_KEY WILLDEEP_API_BASE WILLDEEP_MODEL OPENAI_API_KEY OPENAI_BASE_URL ANTHROPIC_API_KEY ANTHROPIC_BASE_URL].each { |key| env[key] = nil }
results = []
run_case = lambda do |name, args, actions, expected, chooser, minimum = 0|
  bytes = ''.b
  started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  chosen_at = nil
  stage = 0
  workspace_args = args.include?('--workspace') ? [] : ['--workspace', project]
  PTY.spawn(env, binary, '--config', config, *workspace_args, '--language', 'en', *args) do |reader, writer, pid|
    reader.winsize = [30, 100]
    begin
      loop do
        elapsed = Process.clock_gettime(Process::CLOCK_MONOTONIC) - started
        if IO.select([reader], nil, nil, 0.03)
          part = reader.read_nonblock(1_000_000, exception: false)
          break if part.nil?
          unless part == :wait_readable
            bytes << part
            chosen_at ||= elapsed if bytes.include?("project's") && bytes.include?('session?')
            writer.write("\e[6;18;9t\e[0n") if part.include?("\e[16t") || part.include?("\e[5n")
          end
        end
        relative = elapsed - (chosen_at || 0)
        if stage < actions.length && chosen_at && relative >= actions[stage][0]
          writer.write(actions[stage][1])
          stage += 1
        end
        if bytes.include?(expected) && elapsed >= minimum
          raise "#{name}: wrong chooser visibility" unless !!chosen_at == chooser
          raise "#{name}: restored another project's history" if bytes.include?('STARTUP_HISTORY_B') && expected != 'STARTUP_HISTORY_B'
          if name == 'timeout'
            raise 'timeout resumed before 5 seconds' if relative < 4.9
          end
          if name == 'paused-new'
            raise 'paused chooser resumed automatically' if bytes.include?('STARTUP_HISTORY_A')
          end
          # Prove crossterm's EventStream still reads input after the startup reader.
          writer.write("#1983#\r")
          deadline = elapsed + 4
          loop do
            break if bytes.include?('confirmation') || bytes.include?('Confirmation')
            raise "#{name}: main TUI did not receive input" if Process.clock_gettime(Process::CLOCK_MONOTONIC) - started > deadline
            if IO.select([reader], nil, nil, 0.05)
              part = reader.read_nonblock(1_000_000, exception: false)
              bytes << part if part.is_a?(String)
            end
          end
          results << {name: name, passed: true, chooser: !!chosen_at, seconds: elapsed.round(2)}
          break
        end
        raise "#{name}: timed out" if elapsed > 16
      end
    rescue Errno::EIO
      raise "#{name}: TUI exited unexpectedly"
    ensure
      File.binwrite(File.join(root, "#{name}.ansi"), bytes)
      Process.kill('TERM', pid) rescue Errno::ESRCH
      begin
        Timeout.timeout(2) { Process.wait(pid) }
      rescue Timeout::Error
        Process.kill('KILL', pid) rescue Errno::ESRCH
      rescue Errno::ECHILD
      end
    end
  end
end
begin
  run_case.call('timeout', [], [], 'STARTUP_HISTORY_A', true)
  run_case.call('new-key', [], [[0.2, 'N']], 'Hello,', true)
  run_case.call('paused-new', [], [[0.2, "\e[B"], [6, "\r"]], 'Hello,', true)
  # 100x30: centered 90x14 block; action rows are terminal rows 17 and 19.
  run_case.call('mouse-continue', [], [[0.2, "\e[<0;10;17M\e[<0;10;17m"]], 'STARTUP_HISTORY_A', true)
  run_case.call('mouse-new', [], [[0.2, "\e[<0;10;19M\e[<0;10;19m"]], 'Hello,', true)
  run_case.call('single-new', ['--new-session'], [], 'Hello,', false)
  run_case.call('explicit-resume', ['--resume', ids[0]], [], 'STARTUP_HISTORY_A', false)
  run_case.call('fresh-project', ['--workspace', fresh], [], 'Hello,', false)
  system(env, binary, '--config', config, 'config', 'startup', 'latest', out: File::NULL) or raise 'save latest failed'
  run_case.call('preference-latest', [], [], 'STARTUP_HISTORY_A', false)
  system(env, binary, '--config', config, 'config', 'startup', 'new', out: File::NULL) or raise 'save new failed'
  run_case.call('preference-new', [], [], 'Hello,', false)
  run_case.call('override-latest', ['--session-startup', 'latest'], [], 'STARTUP_HISTORY_A', false)
  before.each { |path, messages| raise 'saved history changed' unless JSON.parse(File.read(path))['messages'] == messages }
  raise 'startup issued inference or unexpected requests' unless requests.all? { |line| line.start_with?('GET /v1/model-capabilities ') }
  raise 'startup created blank persisted sessions' unless Dir[File.join(home, 'sessions', '*.json')].size == 3
ensure
  system(env, binary, 'daemon', 'stop', out: File::NULL, err: File::NULL)
  server.close
  listener.join
  inference = requests.count { |line| !line.start_with?('GET /v1/model-capabilities ') }
  report = {root: root, cases: results, metadata_requests: requests.size - inference, model_requests: inference, passed: results.size == 11 && inference.zero?}
  File.write(File.join(root, 'report.json'), JSON.pretty_generate(report))
  File.write(File.join(root, 'report.md'), "# Session startup verification\n\n" + results.map { |r| "- #{r[:name]}: PASS (#{r[:seconds]}s)" }.join("\n") + "\n\nModel inference requests: #{inference}\nMetadata GET requests: #{requests.size - inference}\n")
  puts report.to_json
end
