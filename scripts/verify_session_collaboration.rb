#!/usr/bin/env ruby
# Runs the real Runtime against a loopback-only, deterministic model fixture.
require 'json'
require 'socket'
require 'tmpdir'
require 'fileutils'
require 'open3'
require 'securerandom'
require 'timeout'

ROOT = File.expand_path('..', __dir__)
BIN = ENV.fetch('WILLDEEP_BIN', File.join(ROOT, 'target/debug/willdeep'))
REPORT = File.join(ROOT, 'build/verification/session-collaboration-runtime.json')

class ModelFixture
  attr_accessor :source, :target
  attr_reader :port, :requests

  def initialize
    @server = TCPServer.new('127.0.0.1', 0)
    @port = @server.addr[1]
    @requests = []
    @delegated = false
    @replied = []
    @thread = Thread.new do
      loop do
        socket = @server.accept
        Thread.new(socket) { |client| respond(client) }
      end
    rescue IOError, Errno::EBADF
      nil
    end
  end

  def respond(socket)
    first = socket.gets
    return unless first
    headers = {}
    while (line = socket.gets) && line != "\r\n"
      key, value = line.split(':', 2)
      headers[key.downcase] = value.strip
    end
    body = socket.read(headers.fetch('content-length', '0').to_i)
    if first.start_with?('GET ', 'HEAD ') || body.empty?
      data = { object: 'list', data: [{ id: 'fixture-model', object: 'model', owned_by: 'fixture' }] }
    else
      request = JSON.parse(body)
      @requests << request
      data = completion(request)
    end
    payload = JSON.generate(data)
    socket.write("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: #{payload.bytesize}\r\nConnection: close\r\n\r\n#{payload}")
  rescue StandardError => error
    warn "fixture: #{error.class}: #{error.message}"
  ensure
    socket.close
  end

  def completion(request)
    messages = request.fetch('messages', [])
    text = JSON.generate(messages)
    available = request.fetch('tools', []).any? { |tool| tool.dig('function', 'name') == 'send_session_message' }
    arguments = nil
    if available && text.include?('E2E_WORK') && text.include?('Coordination data from another session')
      note = messages.filter_map do |message|
        content = message['content']
        next unless content.is_a?(String) && content.include?('Coordination data from another session')
        content.lines.filter_map { |line| JSON.parse(line) rescue nil }.find { |item| item.is_a?(Hash) && ['E2E_WORK', 'PHONE_E2E_WORK'].include?(item['body']) }
      end.last
      if note && !@replied.include?(note['id'])
        arguments = { session_id: @source, message: note['body'].sub('WORK', 'REPORT'), reply_to: note.fetch('id') }
        @replied << note['id']
      end
    elsif available && text.include?('E2E_A_START') && !@delegated
      arguments = { session_id: @target, message: 'E2E_WORK' }
      @delegated = true
    end
    message = { role: 'assistant', content: arguments ? nil : 'Fixture finished within the authorized task.' }
    if arguments
      message[:tool_calls] = [{ id: "call_#{SecureRandom.hex(8)}", type: 'function',
        function: { name: 'send_session_message', arguments: JSON.generate(arguments) } }]
    end
    { id: "chatcmpl-#{SecureRandom.hex(8)}", object: 'chat.completion', created: Time.now.to_i,
      model: 'fixture-model', choices: [{ index: 0, message: message, finish_reason: arguments ? 'tool_calls' : 'stop' }],
      usage: { prompt_tokens: 100, completion_tokens: 20, total_tokens: 120 } }
  end

  def close
    @server.close
    @thread.join
  end
end

def call_api(env, operation, params = {})
  output, error, status = Open3.capture3(env, BIN, 'api', operation, '--params-file', '-', '--ndjson', stdin_data: JSON.generate(params))
  raise "#{operation}: #{error}\n#{output}" unless status.success?
  response = JSON.parse(output)
  raise "#{operation}: #{response}" unless response['status'] == 'ok'
  response.fetch('data')
end

def wait_for(description)
  Timeout.timeout(60) do
    loop do
      value = yield
      return value if value
      sleep 0.2
    end
  end
rescue Timeout::Error
  raise "Timed out waiting for #{description}"
end

if $PROGRAM_NAME == __FILE__
fixture = ModelFixture.new
Dir.mktmpdir('wd-peer-', '/private/tmp') do |root|
  home = File.join(root, 'home')
  workspace = File.join(root, 'workspace')
  FileUtils.mkdir_p([home, workspace], mode: 0o700)
  config = "version = 1\ndefault_provider = \"fixture\"\n[agent]\nmax_turns = 8\nsafety_judge = false\n[local_model]\nenabled = false\n[providers.fixture]\nprovider = \"openai-compatible\"\napi = \"chat-completions\"\napi_base = \"http://127.0.0.1:#{fixture.port}/v1\"\napi_key = \"fixture-only\"\nmodel = \"fixture-model\"\n"
  File.write(File.join(home, 'config.toml'), config, perm: 0o600)
  env = { 'WILLDEEP_HOME' => home, 'WILLDEEP_CONFIG' => File.join(home, 'config.toml'),
    'WILLDEEP_API_KEY' => nil, 'WILLDEEP_API_BASE' => nil, 'WILLDEEP_MODEL' => nil }
  begin
    call_api(env, 'workspace.register', root: workspace, name: 'Peer fixture', provider_profile: 'fixture')
    fixture.source = call_api(env, 'session.create', workspace: workspace, title: 'E2E A', profile: 'fixture').fetch('id')
    fixture.target = call_api(env, 'session.create', workspace: workspace, title: 'E2E B', profile: 'fixture').fetch('id')
    submit = lambda do |id, prompt|
      call_api(env, 'turn.submit', session_id: id, turn_request_id: SecureRandom.uuid, prompt: prompt, origin_client: 'fixture-operator')
    end
    submit.call(fixture.target, 'E2E_B_READY: wait for collaboration within this test.')
    wait_for('B idle') { call_api(env, 'session.get', id: fixture.target)['status'] == 'idle' }
    submit.call(fixture.source, 'E2E_A_START: send the fixture task to the other session and wait for its report.')
    notes = wait_for('model reply and both consumption receipts') do
      records = call_api(env, 'session.peer.inbox', session_id: fixture.source)
      records if records.size == 2 && records.all? { |note| note['consumed_at'] } && records.any? { |note| note['body'] == 'E2E_REPORT' && note['reply_to'] }
    end
    raise 'Wrong source binding' unless notes.find { |note| note['body'] == 'E2E_WORK' }['from_session'] == fixture.source
    read = call_api(env, 'session.peer.read', session_id: fixture.source, message_id: notes.find { |note| note['body'] == 'E2E_REPORT' }.fetch('id'), client_id: 'fixture-reviewer')
    binary_version, version_error, version_status = Open3.capture3(BIN, '--version')
    raise version_error unless version_status.success?
    baseline, git_error, git_status = Open3.capture3('git', '-C', ROOT, 'rev-parse', 'HEAD')
    raise git_error unless git_status.success?
    report = { passed: true, binary_version: binary_version.strip, baseline: baseline.strip,
      source: fixture.source, target: fixture.target, notes: notes,
      read_result: read, model_requests: fixture.requests.size, loopback_only: true }
    if ENV['WILLDEEP_ANDROID_DIR']
      android = File.expand_path(ENV.fetch('WILLDEEP_ANDROID_DIR'))
      require File.join(android, 'scripts/collaboration_relay_fixture')
      relay = CollaborationRelayFixture.new('fixture-only-relay')
      begin
        room = 'wd-peerfixture'
        File.write(File.join(home, 'mobile-relay.toml'), "relay_base_url = \"http://127.0.0.1:#{relay.port}\"\nroom = \"#{room}\"\ntoken = \"fixture-only-relay\"\nenabled = true\n", perm: 0o600)
        call_api(env, 'mobile.enable')
        adb = ENV.fetch('ADB', 'adb')
        serial = ENV.fetch('ANDROID_SERIAL', 'emulator-5560')
        name, = Open3.capture3(adb, '-s', serial, 'emu', 'avd', 'name')
        raise 'Unexpected AVD; refusing to install' unless name.lines.first.to_s.strip == 'wd_collaboration_api33'
        [
          ['reverse', "tcp:#{relay.port}", "tcp:#{relay.port}"],
          ['install', '-r', File.join(android, 'app/build/outputs/apk/debug/app-debug.apk')],
          ['install', '-r', File.join(android, 'app/build/outputs/apk/androidTest/debug/app-debug-androidTest.apk')]
        ].each do |args|
          output, error, status = Open3.capture3(adb, '-s', serial, *args)
          raise "adb #{args.first}: #{output} #{error}" unless status.success?
        end
        payload = Base64.strict_encode64(JSON.generate(relay_base_url: "http://127.0.0.1:#{relay.port}", relay_room: room,
          relay_token: 'fixture-only-relay', desktop_name: 'Rust fixture'))
        output, error, status = Open3.capture3(adb, '-s', serial, 'shell', 'am', 'instrument', '-w',
          '-e', 'class', 'com.willdeep.android.SessionCollaborationLiveTest', '-e', 'local_fixture', 'true',
          '-e', 'source', fixture.source, '-e', 'target', fixture.target, '-e', 'work', 'PHONE_E2E_WORK',
          '-e', 'report', 'PHONE_E2E_REPORT', '-e', 'pairing_base64', payload,
          'com.willdeep.android.test/androidx.test.runner.AndroidJUnitRunner')
        FileUtils.mkdir_p(File.dirname(REPORT))
        File.write(File.join(File.dirname(REPORT), 'android-rust-collaboration.txt'), output + error)
        raise "Android live test failed: #{output} #{error}" unless status.success? && output.include?('OK (1 test)')
        report[:android_live] = true
        puts 'PASS: Android real ViewModel/WebSocket → Rust → model reply → mobile inbox/read'
      ensure
        Open3.capture3(adb, '-s', serial, 'reverse', '--remove', "tcp:#{relay.port}") if adb && serial
        relay.close
      end
    end
    FileUtils.mkdir_p(File.dirname(REPORT))
    File.write(REPORT, JSON.pretty_generate(report) + "\n")
    File.write(REPORT.sub(/\.json$/, '.md'), "# 会话协作 Runtime 验证\n\n- 状态：通过\n- 真实 Runtime / 隔离 WILLDEEP_HOME / 本机测试模型\n- A 发送、B 空闲唤醒、关联回复、A 消费、独立已读均通过\n- Android 实机模拟器联调：#{report[:android_live] ? '通过' : '未运行'}\n")
    puts "PASS: A tool send → B idle wake → B model reply → A consumption → inbox/read (#{fixture.requests.size} local model requests)"
  rescue StandardError
    log = File.join(home, 'runtime/daemon.log')
    warn File.read(log).lines.last(25).join if File.file?(log)
    raise
  ensure
    Open3.capture3(env, BIN, 'daemon', 'stop')
  end
end
fixture.close
end
