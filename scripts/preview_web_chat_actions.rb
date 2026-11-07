#!/usr/bin/env ruby
# frozen_string_literal: true
# Start only via the desktop preview manager. Owns a private HOME and local model.
require 'json'
require 'socket'
require 'open3'
require 'timeout'
require 'tmpdir'
require 'fileutils'
require 'securerandom'

LOOPBACK = [127, 0, 0, 1].join('.').freeze

class WebChatActionsProvider
  attr_reader :port

  def initialize
    @server = TCPServer.new(LOOPBACK, 0)
    @port = @server.addr[1]
    @workers = []
    @thread = Thread.new do
      loop do
        client = @server.accept
        @workers << Thread.new(client) do |socket|
          Timeout.timeout(20) { respond(socket) }
        rescue EOFError, IOError, Errno::ECONNRESET, Errno::EPIPE => error
          warn "local fixture connection: #{error.class}"
        ensure
          socket.close
        end
      end
    rescue IOError, Errno::EBADF
      # This fixture alone owns the listener.
    end
  end

  def respond(socket)
    first = socket.gets or return
    headers = {}
    while (line = socket.gets) && line != "\r\n"
      name, value = line.split(':', 2)
      headers[name.downcase] = value.to_s.strip
    end
    if first.start_with?('GET ')
      send_response(socket, 'application/json', JSON.generate(data: [{ id: 'web-chat-fixture', object: 'model' }]))
      return
    end
    request = JSON.parse(socket.read(Integer(headers.fetch('content-length', '0'))))
    messages = Array(request['messages'])
    user_index = messages.rindex { |message| message['role'] == 'user' } || 0
    prompt = messages[user_index]['content'].to_s
    tools = Array(request['tools']).map { |tool| tool.dig('function', 'name') }
    called = messages.drop(user_index).flat_map { |message| Array(message['tool_calls']) }.map { |tool| tool.dig('function', 'name') }
    if prompt.include?('触发具体错误')
      error = { error: { type: 'invalid_request_error', code: 'model_not_found',
        message: 'Requested model preview-missing is unavailable. token=preview-redaction-sentinel /private/tmp/private-workspace/config.toml' } }
      send_response(socket, 'text/event-stream', "data: #{JSON.generate(error)}\n\n")
      return
    end
    call = nil
    text = '普通聊天已完成。'
    if prompt.include?('子会话') && tools.include?('start_session') && !called.include?('start_session')
      call = ['start_session', { title: '核对配置加载顺序', prompt: '阅读 evidence.txt，调查配置加载顺序，报告证据及尚未核实的问题。',
        context: 'fresh', workspace_mode: 'shared_read_only', subscription: 'inbox' }]
    elsif prompt.include?('触发待处理审批') && !called.include?('create_file')
      call = ['create_file', { path: 'approval-evidence.txt', content: 'isolated preview evidence' }]
    elsif prompt.include?('evidence.txt') && tools.include?('read_file') && !called.include?('read_file')
      call = ['read_file', { path: 'evidence.txt' }]
    elsif called.include?('start_session')
      text = '子会话提议已准备，请确认任务说明后点击开始。'
    elsif called.include?('read_file')
      text = '检查完成：已读取 evidence.txt，配置加载顺序为默认配置、项目配置、显式参数。'
    end
    message = { role: 'assistant', content: call ? nil : text }
    if call
      message[:tool_calls] = [{ id: "preview-#{SecureRandom.hex(8)}", type: 'function',
        function: { name: call[0], arguments: JSON.generate(call[1]) } }]
    end
    finish = call ? 'tool_calls' : 'stop'
    if request['stream']
      delta = message.dup
      delta[:tool_calls] = delta[:tool_calls].each_with_index.map { |tool, index| tool.merge(index: index) } if call
      chunks = [{ choices: [{ index: 0, delta: delta, finish_reason: nil }] },
        { choices: [{ index: 0, delta: {}, finish_reason: finish }] }]
      send_response(socket, 'text/event-stream', chunks.map { |chunk| "data: #{JSON.generate(chunk)}\n\n" }.join + "data: [DONE]\n\n")
    else
      send_response(socket, 'application/json', JSON.generate(choices: [{ index: 0, message: message, finish_reason: finish }]))
    end
  end

  def send_response(socket, type, body)
    socket.write("HTTP/1.1 200 OK\r\nContent-Type: #{type}\r\nContent-Length: #{body.bytesize}\r\nConnection: close\r\n\r\n#{body}")
  end

  def close
    @server.close
    @thread.join
    @workers.each(&:join)
  end
end

binary = File.expand_path(ARGV.fetch(0))
port = Integer(ARGV.fetch(1, '19862'))
output = File.expand_path(ARGV.fetch(2, File.join(__dir__, '../build/verification/web-chat-actions')))
root = Dir.mktmpdir('willdeep-web-chat-actions-', '/private/tmp')
home, workspace = File.join(root, 'home'), File.join(root, 'workspace')
FileUtils.mkdir_p([home, workspace, output], mode: 0o700)
File.write(File.join(workspace, 'evidence.txt'), "默认配置 -> 项目配置 -> 显式参数\n")
provider = WebChatActionsProvider.new
config = File.join(home, 'config.toml')
File.write(config, <<~TOML, perm: 0o600)
  version=1
  default_provider="fixture"
  [agent]
  approval="smart"
  max_turns=6
  human_wait_yield_seconds=1
  safety_judge=false
  auto_title=false
  input_suggestions=false
  small_model_routing=false
  auto_dispatch_read_only=false
  [local_model]
  enabled=false
  [providers.fixture]
  provider="openai-compatible"
  api="chat-completions"
  api_base="http://#{LOOPBACK}:#{provider.port}/v1"
  api_key="#{SecureRandom.hex(24)}"
  model="web-chat-fixture"
TOML
env = { 'PATH' => ENV.fetch('PATH'), 'HOME' => home, 'USERPROFILE' => home,
  'WILLDEEP_HOME' => home, 'WILLDEEP_CONFIG' => config, 'WILLDEEP_LANGUAGE' => 'zh-CN',
  'WILLDEEP_LOCAL_DISCOVERY' => '0', 'WILLDEEP_TELEMETRY_DISABLED' => '1', 'WILLDEEP_WORKSPACE_CHECKPOINTS' => '0',
  'XDG_CONFIG_HOME' => File.join(home, '.config'), 'XDG_CACHE_HOME' => File.join(home, '.cache'),
  'XDG_DATA_HOME' => File.join(home, '.local/share'), 'LANG' => 'en_US.UTF-8', 'NO_PROXY' => "#{LOOPBACK},localhost" }
api = lambda do |operation, params|
  stdout, stderr, status = Open3.capture3(env, binary, 'api', operation, '--params-file', '-', '--ndjson',
    stdin_data: JSON.generate(params), unsetenv_others: true)
  envelope = JSON.parse(stdout)
  raise "#{operation}: #{envelope['error']} #{stderr}" unless status.success? && envelope['status'] == 'ok'
  envelope.fetch('data')
end
wait = lambda do |label, &condition|
  Timeout.timeout(60) do
    loop do
      break if condition.call
      sleep 0.2
    end
  end
rescue Timeout::Error
  raise "isolated preview timed out: #{label}"
end
stopping = false
%w[TERM INT].each { |signal| Signal.trap(signal) { stopping = true } }
web = nil
begin
  _, error, status = Open3.capture3(env, binary, 'daemon', 'start', unsetenv_others: true)
  raise "isolated Runtime start failed: #{error}" unless status.success?
  api.call('workspace.register', { root: workspace, name: 'Web 交互验证', access: 'smart', provider_profile: 'fixture' })
  create = lambda { |title| api.call('session.create', { workspace: workspace, profile: 'fixture', title: title }).fetch('id') }
  submit = lambda { |id, prompt| api.call('turn.submit', { session_id: id, prompt: prompt,
    turn_request_id: SecureRandom.uuid, origin_client: 'web:isolated-preview' }) }
  main = create.call('普通聊天与子会话提议')
  normal = submit.call(main, '普通聊天检查')
  wait.call('normal conversation') { api.call('turn.get', { id: normal.fetch('id') })['status'] == 'completed' }
  failed = create.call('模型不可用的失败会话')
  failure = submit.call(failed, '触发具体错误')
  wait.call('failed conversation') { api.call('turn.get', { id: failure.fetch('id') })['status'] == 'failed' }
  approval = create.call('等待确认的配置检查')
  api.call('session.update_approval_mode', { id: approval, approval_mode: 'strict' })
  submit.call(approval, '触发待处理审批')
  wait.call('approval request') { !api.call('approval.list', {}).empty? }
  metadata = { root: root, home: home, workspace: workspace, binary: binary, port: port,
    main: main, failed: failed, approval: approval, url: "http://#{LOOPBACK}:#{port}/?session=#{main}" }
  File.write(File.join(output, 'fixture.json'), JSON.pretty_generate(metadata) + "\n")
  web = Process.spawn(env, binary, '--web', '--listen', "#{LOOPBACK}:#{port}", '-c', config, '-w', workspace, '--language', 'zh-CN', unsetenv_others: true)
  puts "Web chat actions fixture ready: #{metadata[:url]}"
  STDOUT.flush
  until stopping
    if Process.waitpid(web, Process::WNOHANG)
      web = nil
      raise 'isolated Web exited unexpectedly'
    end
    sleep 0.2
  end
ensure
  if web
    begin
      Process.kill('TERM', web)
      Process.waitpid(web)
    rescue Errno::ESRCH, Errno::ECHILD
      # Only reap the child launched by this fixture.
    end
  end
  begin
    api.call('approval.list', {}).each { |gate| api.call('approval.resolve', { id: gate.fetch('id'), decision: 'deny' }) }
    Open3.capture3(env, binary, 'daemon', 'stop', unsetenv_others: true)
  ensure
    provider.close
  end
end
