#!/usr/bin/env ruby
# Two local protocol fixtures, real CLI startup, persisted profiles and TUI selection.
require 'socket'
require 'json'
require 'tmpdir'
require 'fileutils'
require 'pty'
require 'io/console'
require 'open3'
require 'timeout'

binary, output = ARGV.map { |value| File.expand_path(value) }
root = Dir.mktmpdir('wd-local-discovery-', '/private/tmp')
home, workspace = %w[home workspace].map { |name| File.join(root, name) }
FileUtils.mkdir_p([home, workspace, File.dirname(output)])
requests, servers, listeners = [], {}, []
available = {'ollama' => false, 'magpie' => true}
%w[ollama magpie].each do |name|
  server = TCPServer.new('127.0.0.1', 0)
  servers[name] = server
  listeners << Thread.new do
    loop do
      socket = server.accept
      Thread.new(socket) do |client|
        begin
          method, path = client.gets.to_s.split
          headers = {}
          while (line = client.gets) && line != "\r\n"
            key, value = line.split(':', 2)
            headers[key.downcase] = value.strip if value
          end
          body = client.read(headers.fetch('content-length', '0').to_i)
          data = body.empty? ? {} : JSON.parse(body)
          requests << {provider: name, method: method, path: path, model: data['model'], authorization: headers['authorization']}
          response = case path
          when '/api/tags' then {models: [{name: 'local-coder'}]}
          when '/api/hello' then {name: name, version: 'fixture'}
          when '/v1/models' then {data: [{id: 'local-coder'}]}
          else nil
          end
          if !available[name]
            client.write("HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
          elsif method == 'GET' && response
            json = JSON.generate(response)
            client.write("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: #{json.bytesize}\r\nConnection: close\r\n\r\n#{json}")
          elsif method == 'POST' && path == '/v1/chat/completions'
            client.write("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n")
            client.write("data: #{JSON.generate({choices: [{index: 0, delta: {content: "#{name.upcase}_DISCOVERY_OK"}, finish_reason: 'stop'}]})}\n\ndata: [DONE]\n\n")
          else
            client.write("HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
          end
        rescue IOError, Errno::EPIPE, Errno::ECONNRESET
        ensure
          client.close
        end
      end
    end
  rescue IOError, Errno::EBADF
  end
end
config = File.join(home, 'config.toml')
original = "# keep configured cloud default\nversion=1\ndefault_provider='cloud'\n[agent]\nauto_title=false\ninput_suggestions=false\nsmall_model_routing=false\nauto_dispatch_read_only=false\n[ui]\ncheck_updates=false\n[providers.cloud]\nprovider='openai-compatible'\napi='chat-completions'\napi_base='http://127.0.0.1:1/v1'\napi_key='cloud-fixture-never-send'\nmodel='cloud-model'\n"
File.write(config, original, perm: 0600)
env = {'WILLDEEP_HOME' => home, 'TERM' => 'xterm-256color', 'WILLDEEP_TELEMETRY_DISABLED' => '1',
  'WILLDEEP_LOCAL_DISCOVERY' => '1', 'OLLAMA_HOST' => "http://127.0.0.1:#{servers['ollama'].addr[1]}",
  'MAGPIE_ADDR' => "127.0.0.1:#{servers['magpie'].addr[1]}", 'NO_PROXY' => '127.0.0.1,localhost', 'no_proxy' => '127.0.0.1,localhost'}
%w[WILLDEEP_API_KEY WILLDEEP_API_BASE WILLDEEP_MODEL WILLDEEP_CONFIG OPENAI_API_KEY OPENAI_BASE_URL ANTHROPIC_API_KEY ANTHROPIC_BASE_URL].each { |key| env[key] = nil }
bytes, phases = ''.b, []
begin
  PTY.spawn(env, binary, '--config', config, '--workspace', workspace, '--language', 'en', '--new-session') do |reader, writer, pid|
    reader.winsize = [30, 100]
    wait = lambda do |name, &check|
      deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + 25
      loop do
        if IO.select([reader], nil, nil, 0.05)
          part = reader.read_nonblock(1_000_000, exception: false)
          raise 'TUI exited' if part.nil?
          if part.is_a?(String)
            bytes << part
            writer.write("\e[6;18;9t\e[0n") if part.include?("\e[16t") || part.include?("\e[5n")
          end
        end
        File.binwrite(File.join(root, 'screen.ansi'), bytes)
        snapshot, error, status = Open3.capture3('node', File.join(__dir__, 'terminal_snapshot.mjs'), File.join(root, 'screen.ansi'), '', '100', '30')
        raise error unless status.success?
        screen = JSON.parse(snapshot)
        if check.call(screen['text'])
          phases << name
          File.write(File.join(root, "#{name}.json"), snapshot)
          break
        end
        raise "#{name} timed out: #{root}" if Process.clock_gettime(Process::CLOCK_MONOTONIC) > deadline
      end
    end
    begin
      wait.call('ready') { |text| text.include?('Hello,') }
      raise 'original configuration overwritten' unless File.read(config).start_with?(original)
      writer.write("/model\r")
      wait.call('startup-detects-running-magpie') { |text| text.include?('Provider · 2') && text.include?('magpie') && !text.include?('ollama') }
      available['ollama'] = true
      writer.write("\e[15~")
      wait.call('both-local-providers-visible') { |text| text.include?('Provider · 3') && text.include?('magpie') && text.include?('ollama') }
      %w[magpie ollama].each do |name|
        # Initial focus is Models. Each reopen starts at the active model.
        writer.write("\t\e[B")
        wait.call("#{name}-models-loaded") { |text| text.include?("Models · #{name}") && text.include?('local-coder') && !text.include?('Refreshing models') }
        writer.write("\t\r")
        wait.call("#{name}-confirmed") { |text| !text.include?('Select model') && text.include?("#{name} / local-coder") }
        writer.write("discovery-check-#{name}\r")
        wait.call("#{name}-runtime-answer") { |text| text.include?("#{name.upcase}_DISCOVERY_OK") && text.include?('turn finished') }
        writer.write("/model\r")
        wait.call("#{name}-reopen-no-duplicates") { |text| text.include?('Provider · 3') && text.include?('local-coder') && !text.include?('Refreshing models') }
      end
      writer.write("\e")
      raise 'cloud credential leaked to local service' if requests.any? { |request| request[:authorization].to_s.include?('cloud-fixture') }
      raise 'local Provider model mismatch' unless requests.select { |request| request[:method] == 'POST' }.all? { |request| request[:model] == 'local-coder' }
    ensure
      File.binwrite(File.join(root, 'terminal.ansi'), bytes)
      Process.kill('TERM', pid) rescue Errno::ESRCH
      Process.wait(pid) rescue Errno::ECHILD
    end
  end
  report = {passed: true, binary: binary, root: root, phases: phases, requests: requests, default_preserved: true, deduplicated: true}
  File.write(output + '.json', JSON.pretty_generate(report))
  File.write(output + '.md', "# 本地 Provider 发现验收\n\n" + phases.map { |phase| "- PASS #{phase}" }.join("\n") + "\n")
  puts JSON.generate(report)
ensure
  Open3.capture3(env, binary, 'daemon', 'stop')
  servers.each_value(&:close)
  listeners.each { |thread| thread.join(1) }
end
