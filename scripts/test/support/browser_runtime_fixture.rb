#!/usr/bin/env ruby
# frozen_string_literal: true

# Start ONLY through preview_start. This owns one synthetic loopback HTTP server,
# one real Web process and its private Runtime. No real HOME/config/daemon access.
require 'cgi'
require 'optparse'
require_relative 'browser_runtime_support'

class BrowserRuntimeFixture
  attr_reader :origin

  def initialize(root)
    @root, @mutex, @cases, @workers, @sockets, @errors = root, Mutex.new, {}, [], [], []
    @server = TCPServer.new('127.0.0.1', 0)
    @origin = "http://127.0.0.1:#{@server.addr[1]}"
  end

  def start
    @listener = Thread.new do
      loop do
        client = @server.accept
        @sockets << client
        @workers << Thread.new(client) { |socket| serve(socket) }
      end
    rescue IOError, Errno::EBADF
      # Closing the owned listener terminates accept.
    end
  end

  def close
    @server.close unless @server.closed?
    @mutex.synchronize { @cases.each_value { |state| state[:gate].push(:close) } }
    @sockets.each { |socket| socket.close unless socket.closed? }
    @listener&.join(2)
    @workers.each { |worker| worker.join(1); worker.kill if worker.alive? }
  end

  def state(id)
    @mutex.synchronize do
      value = @cases.fetch(id)
      JSON.parse(JSON.generate(value.reject { |key, _| key == :gate }.merge(fixture_errors: @errors)))
    end
  end

  private

  def record(id, key, value)
    @mutex.synchronize { @cases.fetch(id)[key] = value }
  end

  def serve(client)
    Timeout.timeout(BrowserRuntime::DEADLINE * 4) do
      method, target = client.gets.to_s.split
      return unless method
      headers = {}
      while (line = client.gets) && line != "\r\n"
        name, value = line.split(':', 2)
        headers[name.downcase] = value.strip if value
      end
      body = client.read(Integer(headers.fetch('content-length', '0')))
      path, query = target.split('?', 2)
      data = body.empty? ? {} : (headers['content-type'].to_s.include?('application/json') ? JSON.parse(body) : CGI.parse(body))
      if path == '/__qa/register' && method == 'POST'
        id, scenario = data.fetch('id'), data.fetch('scenario')
        BrowserRuntime.assert(id.match?(/\A[a-z0-9-]+\z/) && %w[success approval_cancel executing_cancel].include?(scenario), 'invalid fixture case')
        @mutex.synchronize do
          raise 'duplicate fixture case' if @cases.key?(id)
          @cases[id] = {id: id, scenario: scenario, step: 0, gate: Queue.new, gate_entered: false,
            calls: [], receipts: [], tool_messages: [], page_requests: [], commits: [], hold_entered: false, hold_closed: false}
        end
        respond(client, 'application/json', JSON.generate(registered: id))
      elsif path == '/__qa/state'
        id = CGI.parse(query.to_s).fetch('id').first
        respond(client, 'application/json', JSON.generate(state(id)))
      elsif path == '/__qa/release' && method == 'POST'
        @mutex.synchronize { @cases.fetch(data.fetch('id'))[:gate].push(:release) }
        respond(client, 'application/json', '{}')
      elsif path.end_with?('/models') && method == 'GET'
        respond(client, 'application/json', JSON.generate(data: [{id: 'browser-fixture', object: 'model'}]))
      elsif path.end_with?('/chat/completions') && method == 'POST'
        provider(client, data)
      elsif (match = path.match(%r{\A/case/([a-z0-9-]+)/(form|commit|hold)\z}))
        page(client, method, match[1], match[2], data)
      else
        respond(client, 'text/plain', 'private browser Runtime fixture', code: '404 Not Found')
      end
    end
  rescue IOError, EOFError, Errno::EPIPE, Errno::ECONNRESET
    # The tested Runtime/browser may cancel an owned HTTP connection.
  rescue StandardError => error
    @mutex.synchronize { @errors << "#{error.class}: #{error.message}" }
    warn "browser fixture error: #{error.class}: #{error.message}"
  ensure
    client.close unless client.closed?
  end

  def page(client, method, id, action, data)
    @mutex.synchronize { @cases.fetch(id)[:page_requests] << {method: method, action: action} }
    case action
    when 'form'
      html = "<!doctype html><html><title>Private browser Runtime form</title><body>" \
        "<form method=post action=/case/#{id}/commit><label for=name>Item</label>" \
        '<input id=name name=name><button id=save type=submit>Save</button></form>' \
        '<p id=status>Pending</p></body></html>'
      respond(client, 'text/html; charset=utf-8', html)
    when 'commit'
      value = data.fetch('name').first
      @mutex.synchronize { @cases.fetch(id)[:commits] << {value: value, receipt: "Saved #{id}"} }
      respond(client, 'text/html; charset=utf-8', "<!doctype html><html><title>Receipt</title><body><p id=status>Saved #{id}</p><p>#{CGI.escapeHTML(value)}</p></body></html>")
    when 'hold'
      record(id, :hold_entered, true)
      # Handshake: a real Chrome navigation has reached the local HTTP server.
      # Do not reply. Closing Chrome must close this request's socket naturally.
      loop do
        IO.select([client])
        peek = client.recv(1, Socket::MSG_PEEK)
        if peek.empty?
          record(id, :hold_closed, true)
          break
        end
        raise 'unexpected bytes after complete held navigation request'
      end
    end
  end

  def provider(client, body)
    BrowserRuntime.assert(body['model'] == 'browser-fixture', 'unexpected model/auxiliary Provider call')
    BrowserRuntime.assert(Array(body['tools']).any? { |tool| tool.dig('function', 'name') == 'browser' },
      'normal Runtime Provider request did not expose the browser tool')
    marker = body.fetch('messages').reverse.find { |message| message['role'] == 'user' && message['content'].is_a?(String) && message['content'].include?('BROWSER_ENTRYPOINT ') }
    raise 'missing browser entrypoint marker' unless marker
    id = marker.fetch('content')[/BROWSER_ENTRYPOINT ([a-z0-9-]+)/, 1]
    messages = body.fetch('messages').select { |message| message['role'] == 'tool' }
    parsed = messages.filter_map do |message|
      begin
        value = JSON.parse(message.fetch('content'))
        value if value.is_a?(Hash) && value['tool'] == 'browser'
      rescue JSON::ParserError
        nil # Error messages are retained verbatim below, never treated as receipts.
      end
    end
    current = @mutex.synchronize do
      entry = @cases.fetch(id)
      entry[:tool_messages] = messages
      entry[:receipts] = parsed
      entry[:step] += 1
      entry.dup
    end
    step = current.fetch(:step)
    scenario = current.fetch(:scenario)
    latest = parsed.last
    if step > 1
      BrowserRuntime.assert(latest && latest['action'], "no successful browser receipt before Provider step #{step}: #{messages.last}")
      BrowserRuntime.assert(!JSON.generate(messages.last).include?('UNKNOWN:'), 'UNKNOWN must not be retried')
    end
    result = if scenario == 'success'
      success_step(id, step, latest)
    else
      cancel_step(id, step, latest, current)
    end
    message = if result
      call_id = "#{id}-#{step}"
      @mutex.synchronize { @cases.fetch(id)[:calls] << {id: call_id, arguments: result} }
      {role: 'assistant', tool_calls: [{id: call_id, type: 'function', function: {name: 'browser', arguments: JSON.generate(result)}}]}
    else
      {role: 'assistant', content: "BROWSER_ENTRYPOINT_COMPLETED #{id}"}
    end
    reply(client, body, message)
  end

  def ref(receipt, id)
    BrowserRuntime.assert(receipt['action'] == 'snapshot', 'snapshot required before issuing an element ref')
    node = receipt.fetch('nodes').find { |value| value['id'] == id }
    BrowserRuntime.assert(node && node['ref'], "host snapshot omitted #{id}")
    node.fetch('ref')
  end

  def success_step(id, step, latest)
    case step
    when 1 then {action: 'open', url: "#{origin}/case/#{id}/form"}
    when 2 then {action: 'snapshot'}
    when 3 then {action: 'fill', ref: ref(latest, 'name'), text: "Synthetic #{id}"}
    when 4 then {action: 'snapshot'}
    when 5 then {action: 'click', ref: ref(latest, 'save')}
    when 6 then {action: 'snapshot'}
    when 7 then {action: 'assert', assertion: 'text_equals', ref: ref(latest, 'status'), expected: "Saved #{id}"}
    when 8
      BrowserRuntime.assert(latest['passed'] == true, 'browser business assertion did not pass')
      {action: 'screenshot'}
    when 9
      BrowserRuntime.assert(latest['artifact'].is_a?(Hash), 'no screenshot artifact receipt before close')
      record(id, :gate_entered, true)
      gate = @mutex.synchronize { @cases.fetch(id).fetch(:gate) }
      raise 'fixture shutting down' unless gate.pop == :release
      {action: 'close'}
    when 10
      BrowserRuntime.assert(latest['closed'] == true, 'owned browser was not closed')
      {action: 'status'}
    when 11
      BrowserRuntime.assert(latest['runtime'] == 'not_started' && latest['ready'] == false, 'browser remained started after close')
      nil
    else raise "unexpected extra success Provider step #{step}"
    end
  end

  def cancel_step(id, step, latest, current)
    case step
    when 1 then {action: 'open', url: "#{origin}/case/#{id}/form"}
    when 2 then {action: 'snapshot'}
    when 3
      record(id, :input_ref, ref(latest, 'name'))
      {action: 'screenshot'}
    when 4
      BrowserRuntime.assert(latest['artifact'].is_a?(Hash), 'no committed artifact before cancellation')
      if current[:scenario] == 'approval_cancel'
        record(id, :gate_entered, true)
        raise 'fixture shutting down' unless current.fetch(:gate).pop == :release
        {action: 'fill', ref: state(id).fetch('input_ref'), text: "Must not commit #{id}"}
      else
        {action: 'open', url: "#{origin}/case/#{id}/hold", timeout_seconds: 60}
      end
    else raise "cancelled action was followed by Provider step #{step}"
    end
  end

  def reply(client, body, message)
    finish = message[:tool_calls] ? 'tool_calls' : 'stop'
    if body['stream']
      delta = message.dup
      delta[:tool_calls] = delta[:tool_calls].map.with_index { |call, index| call.merge(index: index) } if delta[:tool_calls]
      chunks = [
        {choices: [{index: 0, delta: delta, finish_reason: nil}]},
        {choices: [{index: 0, delta: {}, finish_reason: finish}], usage: {prompt_tokens: 200, completion_tokens: 20, total_tokens: 220}}
      ].map { |chunk| "data: #{JSON.generate(chunk)}\n\n" }.join + "data: [DONE]\n\n"
      respond(client, 'text/event-stream', chunks)
    else
      respond(client, 'application/json', JSON.generate(id: 'browser-fixture', object: 'chat.completion', model: body['model'],
        choices: [{index: 0, message: message, finish_reason: finish}], usage: {prompt_tokens: 200, completion_tokens: 20, total_tokens: 220}))
    end
  end

  def respond(client, type, text, code: '200 OK')
    client.write("HTTP/1.1 #{code}\r\nContent-Type: #{type}\r\nContent-Length: #{text.bytesize}\r\nConnection: close\r\n\r\n#{text}")
  end
end

if $PROGRAM_NAME == __FILE__
  options = {}
  OptionParser.new do |parser|
    parser.banner = 'preview_start only: ruby browser_runtime_fixture.rb --binary PATH --node PATH --chrome PATH --root NEW_PATH --web-port PORT'
    %w[binary node chrome root].each { |key| parser.on("--#{key} PATH") { |value| options[key] = value } }
    parser.on('--web-port PORT', Integer) { |value| options['web_port'] = value }
  end.parse!
  %w[binary node chrome root web_port].each { |key| options.fetch(key) }
  %w[binary node chrome].each do |key|
    BrowserRuntime.assert(options[key].start_with?('/') && File.executable?(options[key]), "explicit executable required: #{key}")
    options[key] = File.realpath(options[key])
  end
  BrowserRuntime.assert(options['root'].start_with?('/') && !File.exist?(options['root']), '--root must be a new absolute private directory')
  BrowserRuntime.assert(options['web_port'].between?(1024, 65535) && options['web_port'] != 9847, 'private dynamically selected Web port required')
  FileUtils.mkdir_p(options['root'], mode: 0o700)
  root = File.realpath(options['root'])
  environment = BrowserRuntime.environment(root, options['node'])
  workspace, artifacts = File.join(root, 'workspace'), File.join(root, 'artifacts')
  FileUtils.mkdir_p([workspace, artifacts], mode: 0o700)
  # Prevent Git discovery from reaching the checkout that owns target/. All
  # verification snapshots must describe only this disposable workspace.
  BrowserRuntime.command(environment, '/usr/bin/git', '-c', 'init.templateDir=',
    'init', '--quiet', '--initial-branch=fixture', workspace)
  fixture = BrowserRuntimeFixture.new(root)
  config = File.join(environment.fetch('WILLDEEP_HOME'), 'config.toml')
  File.write(config, <<~TOML, perm: 0o600)
    version = 1
    default_provider = "fixture"
    [agent]
    language = "en"
    approval = "full-access"
    sandbox = false
    max_turns = 24
    auto_title = false
    input_suggestions = false
    auto_dispatch_read_only = false
    small_model_routing = false
    [ui]
    mode = "compact"
    check_updates = false
    [providers.fixture]
    provider = "openai-compatible"
    api = "chat-completions"
    api_base = "#{fixture.origin}/v1"
    api_key = "synthetic-local-fixture"
    model = "browser-fixture"
    context_window = 131072
    [browser]
    enabled = true
    node_path = #{JSON.generate(options.fetch('node'))}
    chrome_path = #{JSON.generate(options.fetch('chrome'))}
    artifact_dir = #{JSON.generate(artifacts)}
    allowed_origins = [#{JSON.generate(fixture.origin)}]
    timeout_seconds = 60
  TOML
  manifest = options.merge('root' => root, 'config' => config, 'environment' => environment,
    'workspace' => workspace, 'artifacts' => artifacts, 'fixture_origin' => fixture.origin,
    'web_origin' => "http://127.0.0.1:#{options['web_port']}")
  web = nil
  stopped = Queue.new
  %w[INT TERM].each { |signal| Signal.trap(signal) { stopped.push(signal) } }
  begin
    fixture.start
    BrowserRuntime.command(environment, options['binary'], '--config', config, 'daemon', 'start')
    BrowserRuntime.api(manifest, 'workspace.register', root: workspace, name: 'Private browser acceptance', access: 'full_access')
    BrowserRuntime.api(manifest, 'workspace.set_approval_mode', root: workspace, approval_mode: 'full_access', confirm_full_access: true)
    status = BrowserRuntime.command(environment, options['binary'], 'daemon', 'status')
    runtime_pid = status[/\bpid=(\d+)/, 1]
    BrowserRuntime.assert(runtime_pid, 'private daemon status omitted PID')
    manifest['runtime_pid'] = Integer(runtime_pid)
    manifest['version'] = BrowserRuntime.command(environment, options['binary'], '--version').strip.delete_prefix('willdeep ')
    manifest['binary_sha256'] = Digest::SHA256.file(options['binary']).hexdigest
    web = BrowserRuntime::Command.new(environment, options['binary'], '--config', config,
      '--workspace', workspace, '--web', '--listen', "127.0.0.1:#{options['web_port']}", '--language', 'en')
    BrowserRuntime.await('real Web health') do
      raise "real Web exited: #{web.result}" if web.finished?
      begin
        BrowserRuntime.http(manifest['web_origin'], '/health')
        true
      rescue Errno::ECONNREFUSED
        false
      end
    end
    File.write(File.join(root, 'manifest.json'), JSON.pretty_generate(manifest) + "\n", perm: 0o600)
    $stdout.sync = true
    puts "Browser Runtime fixture ready: #{manifest['web_origin']} manifest=#{root}/manifest.json"
    # Monitor actual Web process termination as well as signal-driven shutdown.
    loop do
      break unless stopped.empty?
      raise "real Web exited unexpectedly: #{web.result}" if web.finished?
      IO.select(nil, nil, nil, 0.1)
    end
  ensure
    web&.close
    begin
      BrowserRuntime.command(environment, options['binary'], 'daemon', 'stop')
    ensure
      fixture.close
    end
  end
end
