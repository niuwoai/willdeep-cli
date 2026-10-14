#!/usr/bin/env ruby
# Isolated local provider + Runtime/Web fixture. Never uses the user's Runtime or credentials.
require 'socket'
require 'json'
require 'tmpdir'
require 'fileutils'
require 'net/http'
require 'securerandom'

binary = File.expand_path(ARGV.fetch(0))
raise 'binary does not exist' unless File.executable?(binary)
reply_delay = Float(ENV.fetch('WILLDEEP_FIXTURE_REPLY_DELAY', '0.1'))
raise 'fixture reply delay must be between 0 and 30 seconds' unless reply_delay.finite? && reply_delay.between?(0, 30)
root = Dir.mktmpdir('wdm-', '/private/tmp')
home = File.join(root, 'home')
workspace = File.join(root, 'workspace')
FileUtils.mkdir_p([home, workspace])
File.write(File.join(workspace, 'fixture.txt'), "isolated budget verification\n")
provider = TCPServer.new('127.0.0.1', 0)
provider_port = provider.addr[1]
port_probe = TCPServer.new('127.0.0.1', 0)
web_port = port_probe.addr[1]
port_probe.close
config = File.join(home, 'config.toml')
File.write(config, "version = 1\ndefault_provider = \"fixture\"\n[providers.fixture]\nprovider = \"openai-compatible\"\napi = \"chat-completions\"\napi_base = \"http://127.0.0.1:#{provider_port}/v1\"\napi_key = \"local-fixture-placeholder\"\nmodel = \"fixture-model\"\n[agent]\nmax_turns = 3\nauto_title = false\ninput_suggestions = false\nsmall_model_routing = false\nauto_dispatch_read_only = false\nsafety_judge = false\n")
File.chmod(0600, config)
environment = %w[WILLDEEP_API_KEY WILLDEEP_API_BASE WILLDEEP_MODEL WILLDEEP_CONFIG OPENAI_API_KEY ANTHROPIC_API_KEY].to_h { |key| [key, nil] }.merge('WILLDEEP_HOME' => home)
requests = []
requests_lock = Mutex.new
Thread.new do
  loop do
    connection = provider.accept
    Thread.new(connection) do |socket|
      begin
        first = socket.gets
        next unless first
        method, target = first.split(' ', 3)
        headers = {}
        while (line = socket.gets) && line != "\r\n"
          key, value = line.split(':', 2)
          headers[key.downcase] = value.strip if value
        end
        body = socket.read(headers.fetch('content-length', '0').to_i)
        parsed = body.empty? ? {} : JSON.parse(body)
        requests_lock.synchronize do
          requests << { method: method, path: target, model: parsed['model'], message_count: parsed.fetch('messages', []).size }
          File.write(File.join(root, 'provider-requests.json'), JSON.pretty_generate(requests))
        end
        if target.start_with?('/v1/models')
          result = JSON.generate(object: 'list', data: [{ id: 'fixture-model', object: 'model' }])
          socket.write("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: #{result.bytesize}\r\nConnection: close\r\n\r\n#{result}")
        elsif parsed['stream']
          socket.write("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n")
          budget_loop = parsed.fetch('messages', []).to_s.include?('budget-loop')
          delta = if budget_loop
            { tool_calls: [{ index: 0, id: "call_#{SecureRandom.hex(4)}", type: 'function', function: { name: 'read_file', arguments: JSON.generate(path: 'fixture.txt') } }] }
          else
            { content: 'Fixture result: ' }
          end
          chunk = { id: 'fixture', object: 'chat.completion.chunk', model: 'fixture-model', choices: [{ index: 0, delta: delta, finish_reason: nil }] }
          socket.write("data: #{JSON.generate(chunk)}\n\n")
          sleep(parsed.fetch('messages', []).to_s.include?('slow') ? 20 : reply_delay)
          chunk[:choices] = [{ index: 0, delta: budget_loop ? {} : { content: 'completed' }, finish_reason: budget_loop ? 'tool_calls' : 'stop' }]
          chunk[:usage] = parsed.fetch('messages', []).to_s.include?('token-budget-loop') ? { prompt_tokens: 1100, completion_tokens: 3, total_tokens: 1103 } : { prompt_tokens: 20, completion_tokens: 3, total_tokens: 23 }
          socket.write("data: #{JSON.generate(chunk)}\n\ndata: [DONE]\n\n")
        else
          result = JSON.generate(id: 'fixture', model: 'fixture-model', choices: [{ index: 0, message: { role: 'assistant', content: 'Fixture result: completed' }, finish_reason: 'stop' }], usage: { prompt_tokens: 20, completion_tokens: 3, total_tokens: 23 })
          socket.write("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: #{result.bytesize}\r\nConnection: close\r\n\r\n#{result}")
        end
      rescue Errno::EPIPE, Errno::ECONNRESET
        warn 'fixture provider connection cancelled by client'
      rescue StandardError => error
        warn "fixture provider error: #{error.class}: #{error.message}"
      ensure
        socket.close
      end
    end
  end
rescue IOError
  # The owned fixture is shutting down.
end
log = File.open(File.join(root, 'web.log'), 'w')
pid = Process.spawn(environment, binary, '--web', '--listen', "127.0.0.1:#{web_port}", '--workspace', workspace, '--config', config, '--profile', 'fixture', out: log, err: log, pgroup: true)
cleanup = lambda do
  Process.kill('TERM', -pid) rescue Errno::ESRCH
  Process.wait(pid) rescue Errno::ECHILD
  system(environment, binary, 'daemon', 'stop', out: File::NULL, err: File::NULL)
  provider.close
end
trap('TERM') { cleanup.call; exit }
trap('INT') { cleanup.call; exit }
ready = false
100.times do
  begin
    response = Net::HTTP.get_response(URI("http://127.0.0.1:#{web_port}/health"))
    ready = response.is_a?(Net::HTTPSuccess)
    break if ready
  rescue Errno::ECONNREFUSED
    # Bounded startup polling, not a model/network retry.
  end
  break if Process.waitpid(pid, Process::WNOHANG)
  sleep 0.1
end
unless ready
  cleanup.call
  warn "fixture Web startup failed; log: #{File.join(root, 'web.log')}"
  exit 1
end
metadata = { root: root, home: home, workspace: workspace, web_url: "http://127.0.0.1:#{web_port}", config: config, pid: pid, fixture_pid: Process.pid }
File.write(File.join(root, 'fixture.json'), JSON.pretty_generate(metadata))
STDOUT.puts(JSON.generate(metadata))
STDOUT.flush
sleep 1 while true
