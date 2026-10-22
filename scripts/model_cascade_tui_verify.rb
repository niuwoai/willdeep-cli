#!/usr/bin/env ruby
# Real TUI/Runtime, isolated home, two authenticated loopback Providers.
require 'socket'
require 'json'
require 'tmpdir'
require 'fileutils'
require 'pty'
require 'io/console'
require 'open3'
require 'timeout'

binary = File.expand_path(ARGV.fetch(0))
output = File.expand_path(ARGV.fetch(1))
root = Dir.mktmpdir('wd-model-cascade-', '/private/tmp')
home, workspace = %w[home workspace].map { |n| File.join(root, n) }
FileUtils.mkdir_p([home, workspace, File.dirname(output)])
server = TCPServer.new('127.0.0.1', 0)
requests, sockets, workers, phases = [], [], [], []
config = File.join(home, 'config.toml')
lines = ['version = 1', "default_provider = 'alpha'", '[agent]', 'max_turns = 2',
  'auto_title = false', 'input_suggestions = false', 'small_model_routing = false',
  'auto_dispatch_read_only = false', '[ui]', "mode = 'compact'", 'check_updates = false']
%w[alpha beta].each do |name|
  lines += ["[providers.#{name}]", "provider = 'openai-compatible'", "api = 'chat-completions'",
    "api_base = 'http://127.0.0.1:#{server.addr[1]}/#{name}/v1'", "api_key = '#{name}-fixture-only'", "model = '#{name}-model'"]
end
File.write(config, lines.join("\n") + "\n", perm: 0600)
original_config = File.binread(config)
listener = Thread.new do
  loop do
    socket = server.accept
    sockets << socket
    workers << Thread.new(socket) do |client|
      begin
        method, path = client.gets.to_s.split
        headers = {}
        while (line = client.gets) && line != "\r\n"
          key, value = line.split(':', 2)
          headers[key.downcase] = value.strip if value
        end
        body = client.read(headers.fetch('content-length', '0').to_i)
        name = path.start_with?('/beta/') ? 'beta' : 'alpha'
        request = body.empty? ? {} : JSON.parse(body)
        record = {method: method, provider: name, path: path, model: request['model'], auth_ok: headers['authorization'] == "Bearer #{name}-fixture-only"}
        requests << record
        if method == 'GET' && path.end_with?('/models')
          sleep(name == 'beta' ? 0.45 : 0.25)
          response = JSON.generate({data: ["#{name}-model", "#{name}-coder"].map { |id| {id: id} }})
          client.write("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: #{response.bytesize}\r\nConnection: close\r\n\r\n#{response}")
        elsif method == 'POST' && path.end_with?('/chat/completions')
          count = requests.count { |r| r[:method] == 'POST' }
          text = "#{name.upcase}_CASCADE_OK_#{count}"
          client.write("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n")
          client.write("data: #{JSON.generate({choices: [{index: 0, delta: {content: text}, finish_reason: 'stop'}]})}\n\ndata: [DONE]\n\n")
        else
          client.write("HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        end
      rescue IOError, Errno::EPIPE, Errno::ECONNRESET
      ensure
        client.close unless client.closed?
      end
    end
  end
rescue IOError, Errno::EBADF
end
env = {'WILLDEEP_HOME' => home, 'TERM' => 'xterm-256color', 'WILLDEEP_TELEMETRY_DISABLED' => '1', 'WILLDEEP_LOCAL_DISCOVERY' => '0',
  'NO_PROXY' => '127.0.0.1,localhost', 'no_proxy' => '127.0.0.1,localhost'}
%w[WILLDEEP_API_KEY WILLDEEP_API_BASE WILLDEEP_MODEL WILLDEEP_CONFIG OPENAI_API_KEY OPENAI_BASE_URL ANTHROPIC_API_KEY ANTHROPIC_BASE_URL].each { |k| env[k] = nil }
bytes = ''.b
cols, rows = 120, 35
begin
  PTY.spawn(env, binary, '--config', config, '--workspace', workspace, '--language', 'en', '--new-session') do |reader, writer, pid|
    reader.winsize = [rows, cols]
    pump = lambda do |seconds|
      until_time = Process.clock_gettime(Process::CLOCK_MONOTONIC) + seconds
      while Process.clock_gettime(Process::CLOCK_MONOTONIC) < until_time
        next unless IO.select([reader], nil, nil, 0.02)
        part = reader.read_nonblock(1_000_000, exception: false)
        raise 'TUI exited unexpectedly' if part.nil?
        next unless part.is_a?(String)
        bytes << part
        writer.write("\e[6;18;9t\e[0n") if part.include?("\e[16t") || part.include?("\e[5n")
      end
    end
    snapshot = lambda do
      path = File.join(root, 'screen.ansi')
      File.binwrite(path, bytes)
      out, error, status = Open3.capture3('node', File.join(__dir__, 'terminal_snapshot.mjs'), path, '', cols.to_s, rows.to_s)
      raise error unless status.success?
      JSON.parse(out)
    end
    wait = lambda do |name, &check|
      deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + 25
      loop do
        pump.call(0.1)
        screen = snapshot.call
        if check.call(screen)
          phases << {name: name, passed: true}
          File.write(File.join(root, "#{name}.json"), JSON.pretty_generate(screen))
          break screen
        end
        raise "#{name}: timeout; #{root}" if Process.clock_gettime(Process::CLOCK_MONOTONIC) > deadline
      end
    end
    loaded = ->(screen, name) { screen['text'].include?('Select model') && screen['text'].include?("#{name}-model") && !screen['text'].include?('Refreshing models') }
    assert_profile = lambda do |expected|
      sessions = Dir[File.join(home, 'sessions', '*.json')].map { |p| JSON.parse(File.read(p)) }
      raise "Session profile/model mismatch: expected #{expected}" unless sessions.all? { |s| s['profile'] == expected && (s['model'].nil? || s['model'] == "#{expected}-model") }
      if File.file?(File.join(home, 'runtime/sessions.json'))
        runtime_sessions = JSON.parse(File.read(File.join(home, 'runtime/sessions.json')))
        raise 'Runtime profile/model mismatch' unless runtime_sessions.all? { |s| s['profile'] == expected && s['model'] == "#{expected}-model" }
      end
      raise 'model browsing modified configuration' unless File.binread(config) == original_config
    end
    click = lambda do |provider_pane, index|
      popup_x = (cols - [cols, 104].min) / 2
      popup_y = (rows - [rows, 24].min) / 2
      x = popup_x + (cols >= 76 && !provider_pane ? 32 : 4)
      y = popup_y + 4 + index
      writer.write("\e[<0;#{x + 1};#{y + 1}M\e[<0;#{x + 1};#{y + 1}m")
    end
    begin
      wait.call('ready') { |s| s['text'].include?('Hello,') }
      writer.write("/model\r")
      wait.call('wide-two-panes-current-model') { |s| loaded.call(s, 'alpha') && s['text'].include?('Provider · 2') }
      writer.write("\t\e[B")
      wait.call('keyboard-provider-links-models') { |s| loaded.call(s, 'beta') }
      assert_profile.call('alpha')
      writer.write("\t\e[B\r")
      wait.call('keyboard-confirmed-beta') { |s| !s['text'].include?('Select model') && s['text'].include?('beta-model') }
      assert_profile.call('beta')
      writer.write("runtime-cascade\r")
      wait.call('runtime-uses-beta') { |s| s['text'].include?('BETA_CASCADE_OK_1') && s['text'].include?('turn finished') }
      writer.write("/local local-cascade\r")
      wait.call('foreground-uses-beta') { |s| s['text'].include?('BETA_CASCADE_OK_2') }
      writer.write("/model\r")
      wait.call('reopened-beta') { |s| loaded.call(s, 'beta') }
      click.call(true, 0)
      wait.call('mouse-provider-links-alpha') { |s| loaded.call(s, 'alpha') }
      click.call(true, 1)
      wait.call('cached-beta-is-visible-while-refreshing') { |s| s['text'].include?('beta-model') && s['text'].include?('Refreshing models') }
      assert_profile.call('beta')
      click.call(true, 0)
      wait.call('mouse-alpha-ready') { |s| loaded.call(s, 'alpha') }
      click.call(false, 1)
      wait.call('mouse-confirmed-alpha') { |s| !s['text'].include?('Select model') && s['text'].include?('alpha-model') }
      assert_profile.call('alpha')
      cols, rows = 52, 16
      reader.winsize = [rows, cols]
      pump.call(0.3)
      writer.write("/model\r")
      wait.call('narrow-model-page') { |s| loaded.call(s, 'alpha') && !s['text'].include?('Provider · 2') }
      writer.write("\e")
      wait.call('narrow-esc-returns-to-provider') { |s| s['text'].include?('Provider · 2') && s['text'].include?('beta') }
      click.call(true, 1)
      wait.call('narrow-mouse-enters-beta-models') { |s| loaded.call(s, 'beta') && !s['text'].include?('Provider · 2') }
      assert_profile.call('alpha')
      writer.write("\e[B\r")
      wait.call('narrow-confirmed-beta') { |s| !s['text'].include?('Select model') }
      assert_profile.call('beta')
      cols, rows = 120, 35
      reader.winsize = [rows, cols]
      pump.call(0.3)
      writer.write("/model\r")
      wait.call('wide-layout-restored') { |s| loaded.call(s, 'beta') && s['text'].include?('Provider · 2') }
      click.call(true, 0)
      wait.call('browse-alpha-before-cancel') { |s| loaded.call(s, 'alpha') }
      writer.write("\e")
      wait.call('cancel-preserves-beta') { |s| !s['text'].include?('Select model') }
      assert_profile.call('beta')
      raise 'incorrect Provider authentication or model' unless requests.all? { |r| r[:auth_ok] && (r[:method] != 'POST' || r[:model] == "#{r[:provider]}-model") }
    ensure
      File.binwrite(File.join(root, 'terminal.ansi'), bytes)
      Process.kill('TERM', pid) rescue Errno::ESRCH
      begin
        Timeout.timeout(3) { Process.wait(pid) }
      rescue Timeout::Error
        Process.kill('KILL', pid) rescue Errno::ESRCH
      rescue Errno::ECHILD
      end
    end
  end
  report = {passed: true, binary: binary, root: root, phases: phases, requests: requests, auth_and_models_correct: true}
  File.write(output + '.json', JSON.pretty_generate(report))
  File.write(output + '.md', "# 两栏模型选择真实 TUI 验收\n\n" + phases.map { |p| "- #{p[:name]}: PASS" }.join("\n") + "\n")
  puts JSON.generate(report)
ensure
  Open3.capture3(env, binary, 'daemon', 'stop')
  server.close
  listener.join(1)
  listener.kill if listener.alive?
  sockets.each { |s| s.close unless s.closed? }
  workers.each { |w| w.join(1); w.kill if w.alive? }
end
