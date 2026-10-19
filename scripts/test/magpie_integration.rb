#!/usr/bin/env ruby
# Real Magpie gateway + deterministic local upstream; no live accounts or paid calls.
require 'json'
require 'socket'
require 'net/http'
require 'open3'
require 'timeout'
require 'tmpdir'
require 'fileutils'

class MockGatewayUpstream
  attr_reader :port, :requests
  def initialize
    @server = TCPServer.new('127.0.0.1', 0)
    @port = @server.addr[1]
    @requests = []
    @lock = Mutex.new
    @threads = []
    @thread = Thread.new do
      loop do
        socket = @server.accept
        @threads << Thread.new(socket) do |client|
          begin
            Timeout.timeout(15) { respond(client) }
          rescue EOFError, IOError, Errno::ECONNRESET, Errno::EPIPE, Timeout::Error
            # The client may cancel a catalog refresh while the fixture shuts down.
          ensure
            client.close
          end
        end
      end
    rescue IOError, Errno::EBADF
      # Closing our listener is the fixture's normal shutdown.
    end
  end

  def close
    @server.close
    @thread.join
    @threads.each(&:join)
  end

  def respond(client)
    line = client.gets or return
    method, path = line.split
    headers = {}
    while (line = client.gets) && line != "\r\n"
      name, value = line.split(':', 2)
      headers[name.downcase] = value.to_s.strip
    end
    body = client.read(headers.fetch('content-length', '0').to_i)
    parsed = body.empty? ? {} : JSON.parse(body)
    @lock.synchronize { @requests << { path: path, body: parsed, auth: headers['authorization'], key: headers['x-api-key'] } }
    if method == 'GET'
      data = %w[chat child claude-mock].map { |id| { id: id, object: 'model', owned_by: 'mock' } }
      return send_response(client, 'application/json', JSON.generate(object: 'list', data: data))
    end
    if path == '/v1/messages'
      reply = { id: 'msg_mock', type: 'message', role: 'assistant', model: parsed['model'], content: [{ type: 'text', text: 'gateway-ok' }], stop_reason: 'end_turn', usage: { input_tokens: 10, output_tokens: 4 } }
      return send_response(client, 'application/json', JSON.generate(reply)) unless parsed['stream']
      events = [
        ['message_start', { type: 'message_start', message: reply.merge(content: [], stop_reason: nil) }],
        ['content_block_start', { type: 'content_block_start', index: 0, content_block: { type: 'text', text: '' } }],
        ['content_block_delta', { type: 'content_block_delta', index: 0, delta: { type: 'text_delta', text: 'gateway-ok' } }],
        ['content_block_stop', { type: 'content_block_stop', index: 0 }],
        ['message_delta', { type: 'message_delta', delta: { stop_reason: 'end_turn', stop_sequence: nil }, usage: { output_tokens: 4 } }],
        ['message_stop', { type: 'message_stop' }]
      ].map { |event, data| "event: #{event}\ndata: #{JSON.generate(data)}\n\n" }.join
      return send_response(client, 'text/event-stream', events)
    end
    messages = parsed.fetch('messages', [])
    has_result = messages.any? { |message| message['role'] == 'tool' }
    prompt = JSON.generate(messages)
    tool = if parsed['model'] != 'child' && !has_result
      if prompt.include?('DELEGATE_TEST')
        { name: 'spawn_agent', arguments: JSON.generate(profile: 'scout', prompt: 'CHILD_TEST', run_in_background: false) }
      elsif prompt.include?('READ_TEST')
        { name: 'read_file', arguments: JSON.generate(path: 'probe.txt') }
      end
    end
    call = tool && { id: 'call_mock', type: 'function', function: tool }
    message = tool ? { role: 'assistant', content: nil, tool_calls: [call] } : { role: 'assistant', content: 'gateway-ok' }
    finish = tool ? 'tool_calls' : 'stop'
    reply = { id: 'chat_mock', object: 'chat.completion', model: parsed['model'], choices: [{ index: 0, message: message, finish_reason: finish }], usage: { prompt_tokens: 10, completion_tokens: 4, total_tokens: 14 } }
    return send_response(client, 'application/json', JSON.generate(reply)) unless parsed['stream']
    delta = tool ? { role: 'assistant', tool_calls: [call.merge(index: 0)] } : { role: 'assistant', content: 'gateway-ok' }
    chunks = [
      { id: 'chat_mock', object: 'chat.completion.chunk', model: parsed['model'], choices: [{ index: 0, delta: delta, finish_reason: nil }] },
      { id: 'chat_mock', object: 'chat.completion.chunk', model: parsed['model'], choices: [{ index: 0, delta: {}, finish_reason: finish }], usage: reply[:usage] }
    ].map { |chunk| "data: #{JSON.generate(chunk)}\n\n" }.join + "data: [DONE]\n\n"
    send_response(client, 'text/event-stream', chunks)
  end

  def send_response(client, type, body)
    client.write("HTTP/1.1 200 OK\r\nContent-Type: #{type}\r\nContent-Length: #{body.bytesize}\r\nConnection: close\r\n\r\n#{body}")
  end
end

def run_command(env, *args)
  Open3.popen3(env, *args, pgroup: true) do |input, output, error, process|
    input.close
    stdout = Thread.new { output.read }
    stderr = Thread.new { error.read }
    begin
      status = Timeout.timeout(90) { process.value }
      text, diagnostics = stdout.value, stderr.value
      raise "#{File.basename(args[0])} #{args[1]} failed: #{diagnostics[0, 700]}" unless status.success?
      text
    ensure
      if process.alive?
        Process.kill('TERM', -process.pid)
        process.join
      end
    end
  end
end

def get_json(url)
  uri = URI(url)
  Net::HTTP.start(uri.host, uri.port, nil, nil, nil, nil, open_timeout: 2, read_timeout: 3) do |http|
    response = http.get(uri.request_uri, { 'Authorization' => 'Bearer magpie-willdeep' })
    raise "gateway returned HTTP #{response.code}" unless response.code == '200'
    JSON.parse(response.body)
  end
end

magpie = File.expand_path(ENV.fetch('MAGPIE_TEST_BIN'))
willdeep = File.expand_path(ENV.fetch('WILLDEEP_TEST_BIN'))
report_dir = File.expand_path(ARGV.fetch(0, '/private/tmp/willdeep-magpie-qa'))
FileUtils.mkdir_p(report_dir)
checks = []
root = Dir.mktmpdir('wdmg-', '/private/tmp')
upstream = MockGatewayUpstream.new
listener = TCPServer.new('127.0.0.1', 0)
gateway_port = listener.addr[1]
listener.close
gateway = "http://127.0.0.1:#{gateway_port}"
gateway_env = {
  'HOME' => File.join(root, 'isolated-user'),
  'XDG_CONFIG_HOME' => File.join(root, 'config'), 'XDG_CACHE_HOME' => File.join(root, 'cache'),
  'XDG_DATA_HOME' => File.join(root, 'data'), 'XDG_STATE_HOME' => File.join(root, 'state'),
  'MAGPIE_NO_ACCOUNTS' => '1', 'DO_NOT_TRACK' => '1', 'MAGPIE_NO_STATS' => '1',
  'MAGPIE_ADDR' => "127.0.0.1:#{gateway_port}"
}
FileUtils.mkdir_p(gateway_env['HOME'])
gateway_log = File.open(File.join(root, 'gateway.log'), 'w')
homes = []
begin
  run_command(gateway_env, magpie, 'provider', 'add', 'Mock', 'id=mock', "url=http://127.0.0.1:#{upstream.port}/v1", "anthropic=http://127.0.0.1:#{upstream.port}", 'key=mock-upstream', 'models=chat,child,claude-mock')
  gateway_pid = Process.spawn(gateway_env, magpie, 'serve', out: gateway_log, err: gateway_log, pgroup: true)
  Timeout.timeout(20) do
    loop do
      begin
        catalog = get_json("#{gateway}/v1/models")
        raise 'model catalog missing mock/chat' unless catalog.fetch('data').any? { |model| model['id'] == 'mock/chat' }
        break
      rescue Errno::ECONNREFUSED
        sleep 0.1
      end
    end
  end
  checks << { name: 'real_magpie_model_catalog', passed: true }
  %w[foreground_openai foreground_anthropic profile_local runtime].each do |name|
    home = File.join(root, name)
    homes << home
    workspace = File.join(home, 'workspace')
    FileUtils.mkdir_p(workspace)
    File.write(File.join(workspace, 'probe.txt'), 'fixture-read-through-magpie')
    env = %w[WILLDEEP_CONFIG WILLDEEP_API_BASE WILLDEEP_API_KEY WILLDEEP_MODEL OPENAI_BASE_URL OPENAI_API_KEY ANTHROPIC_BASE_URL ANTHROPIC_API_KEY SOMEIM_API_KEY].to_h { |key| [key, nil] }
    env.merge!('WILLDEEP_HOME' => home, 'MAGPIE_BASE' => "#{gateway}/v1", 'MAGPIE_API_KEY' => 'magpie-willdeep')
    config = "version=1\n[agent]\nauto_title=false\nmax_turns=6\napproval='smart'\n"
    if name.start_with?('foreground')
      if name == 'foreground_anthropic'
        env.merge!('ANTHROPIC_BASE_URL' => gateway, 'ANTHROPIC_API_KEY' => 'magpie-willdeep', 'WILLDEEP_MODEL' => 'mock/claude-mock')
      else
        env.merge!('OPENAI_BASE_URL' => "#{gateway}/v1", 'OPENAI_API_KEY' => 'magpie-willdeep', 'WILLDEEP_MODEL' => 'mock/chat')
      end
    else
      config = "version=1\ndefault_provider='magpie'\n" + config.split("\n", 2).last
      config += "\n[providers.magpie]\nprovider='openai-compatible'\napi='chat-completions'\napi_base_env='MAGPIE_BASE'\napi_key_env='MAGPIE_API_KEY'\nmodel='mock/chat'\n"
      config += "\n[providers.worker]\nprovider='openai-compatible'\napi_base_env='MAGPIE_BASE'\napi_key_env='MAGPIE_API_KEY'\nmodel='mock/child'\n[subagents.scout]\nprovider_profile='worker'\n"
    end
    File.write(File.join(home, 'config.toml'), config)
    start = upstream.requests.length
    args = ['run', '--workspace', workspace, '--output', 'ndjson']
    args << '--local' unless name == 'runtime'
    prompt = name == 'foreground_anthropic' ? 'ANTHROPIC_TEST' : (name == 'foreground_openai' ? 'READ_TEST' : 'DELEGATE_TEST')
    out = run_command(env, willdeep, *args, prompt)
    events = out.lines.map { |line| JSON.parse(line) }
    raise "#{name}: no completed turn" unless events.any? { |event| event['type'] == 'completed' }
    raise "#{name}: no streamed text" unless events.any? { |event| event['type'] == 'assistant_text_delta' && event['text'].include?('gateway-ok') }
    requests = upstream.requests.drop(start).select { |request| request[:path] != '/v1/models' }
    raise "#{name}: no upstream request" if requests.empty?
    raise "#{name}: upstream authentication was not replaced" unless requests.all? { |request| request[:auth] == 'Bearer mock-upstream' || request[:key] == 'mock-upstream' }
    if prompt == 'READ_TEST'
      raise 'read_file roundtrip missing' unless requests.any? { |request| JSON.generate(request[:body]['messages']).include?('fixture-read-through-magpie') }
    elsif prompt == 'DELEGATE_TEST'
      raise 'explicit child did not use gateway' unless requests.any? { |request| request[:body]['model'] == 'child' }
    end
    if name == 'runtime'
      tasks = JSON.parse(run_command(env, willdeep, 'api', 'task.list'))
      raise 'Runtime task evidence missing' if tasks.fetch('data').empty?
    end
    checks << { name: name, passed: true, requests: requests.size, paths: requests.map { |request| request[:path] }.uniq, models: requests.map { |request| request[:body]['model'] }.uniq }
  end
rescue StandardError => error
  checks << { name: 'integration', passed: false, error: error.message }
ensure
  homes.each do |home|
    run_command({ 'WILLDEEP_HOME' => home }, willdeep, 'daemon', 'stop')
  rescue StandardError => error
    warn "fixture Runtime cleanup: #{error.message}"
    checks << { name: 'fixture_runtime_cleanup', passed: false, error: error.message }
  end
  if gateway_pid
    Process.kill('TERM', -gateway_pid)
    Process.wait(gateway_pid)
  end
  gateway_log.close
  upstream.close
  passed = checks.none? { |check| !check[:passed] }
  report = { passed: passed, willdeep_version: run_command({}, willdeep, '--version').strip,
             magpie_revision: ENV['MAGPIE_TEST_REVISION'], paid_requests: 0, live_credentials_used: false, checks: checks }
  File.write(File.join(report_dir, 'magpie-integration.json'), JSON.pretty_generate(report) + "\n")
  File.write(File.join(report_dir, 'magpie-integration.md'), (["# Magpie 隔离联调", '', "结果：#{passed ? '通过' : '失败'}；付费请求：0；未使用真实账号。", '', '| 场景 | 结果 |', '| --- | --- |'] + checks.map { |check| "| #{check[:name]} | #{check[:passed] ? '通过' : '失败'} |" }).join("\n") + "\n")
  puts JSON.generate(report)
end
exit(checks.all? { |check| check[:passed] } ? 0 : 1)
