#!/usr/bin/env ruby
# Real TUI + real Runtime, isolated HOME, local Provider and gated control proxy.
# No user credentials/configuration/terminals are read or used by this fixture.
require 'socket'
require 'json'
require 'tmpdir'
require 'fileutils'
require 'pty'
require 'io/console'
require 'open3'
require 'timeout'
require 'base64'

class FeedbackGate
  attr_reader :hits
  def initialize
    @mutex, @condition, @hits = Mutex.new, ConditionVariable.new, []
  end
  def arm(operation, failure: false)
    @mutex.synchronize { @rule = {operation: operation, failure: failure, released: false, hits: 0} }
  end
  def intercept(operation)
    @mutex.synchronize do
      rule = @rule
      return false unless rule && rule[:operation] == operation
      rule[:hits] += 1
      @hits << operation
      @condition.wait(@mutex) until rule[:released]
      rule[:failure]
    end
  end
  def entered?
    @mutex.synchronize { @rule && @rule[:hits].positive? }
  end
  def release
    @mutex.synchronize { @rule[:released] = true if @rule; @condition.broadcast }
  end
end

class FeedbackHttpServer
  attr_reader :port, :requests
  attr_accessor :hold_provider
  def initialize(gate: nil, upstream: nil)
    @gate, @upstream = gate, upstream
    @server = TCPServer.new('127.0.0.1', 0)
    @port, @workers, @sockets, @requests = @server.addr[1], [], [], []
    @hold_provider = false
    @listener = Thread.new do
      loop do
        client = @server.accept
        @sockets << client
        @workers << Thread.new(client) { |socket| serve(socket) }
      end
    rescue IOError, Errno::EBADF
    end
  end
  def serve(client)
    line = client.gets
    return unless line
    method, path = line.split
    headers = []
    while (line = client.gets) && line != "\r\n"
      headers << line
    end
    length = headers.find { |h| h.downcase.start_with?('content-length:') }.to_s.split(':', 2)[1].to_i
    body = client.read(length)
    payload = body.empty? ? {} : JSON.parse(body)
    operation = payload['op'] || payload['operation']
    @requests << {method: method, path: path, operation: operation, model: payload['model'], params: payload['params']}
    if @upstream
      if @gate.intercept(operation)
        respond(client, '503 Service Unavailable', 'application/json', JSON.generate({error: 'fixture control rejection'}))
        return
      end
      upstream = TCPSocket.new(*@upstream)
      @sockets << upstream
      forwarded = headers.reject { |h| h.downcase.start_with?('connection:') }.join
      upstream.write("#{method} #{path} HTTP/1.1\r\n#{forwarded}Connection: close\r\n\r\n#{body}")
      IO.copy_stream(upstream, client)
    elsif method == 'GET' && path.end_with?('/models')
      name = path.start_with?('/beta/') ? 'beta' : 'alpha'
      respond(client, '200 OK', 'application/json', JSON.generate({data: %w[a b c].map { |suffix| {id: "#{name}-#{suffix}"} }}))
    elsif method == 'POST' && path.end_with?('/chat/completions')
      sleep 0.02 while @hold_provider
      answer = 'FIXTURE_FEEDBACK_REPLY'
      text = "data: #{JSON.generate({choices: [{index: 0, delta: {content: answer}, finish_reason: 'stop'}]})}\n\ndata: [DONE]\n\n"
      respond(client, '200 OK', 'text/event-stream', text)
    else
      respond(client, '404 Not Found', 'text/plain', '')
    end
  rescue IOError, Errno::EPIPE, Errno::ECONNRESET
  rescue StandardError => error
    warn "fixture HTTP failure: #{error.class}: #{error.message}"
  ensure
    upstream&.close unless upstream&.closed?
    client.close unless client.closed?
  end
  def respond(client, status, type, body)
    client.write("HTTP/1.1 #{status}\r\nContent-Type: #{type}\r\nContent-Length: #{body.bytesize}\r\nConnection: close\r\n\r\n#{body}")
  end
  def close
    @hold_provider = false
    @gate&.release
    @server.close
    @sockets.each { |s| s.close unless s.closed? }
    @listener.join(1)
    @listener.kill if @listener.alive?
    @workers.each { |w| w.join(0.1); w.kill if w.alive? }
  end
end

class FeedbackPty
  COLS, ROWS = 120, 36
  attr_reader :screens, :raw
  def initialize(binary, environment, config, workspace)
    @reader, @writer, @pid = PTY.spawn(environment, binary, '--config', config, '--workspace', workspace,
      '--language', 'en', '--new-session', unsetenv_others: true)
    @reader.winsize = [ROWS, COLS]
    @decoder_in, @decoder_out, @decoder_err, @decoder_wait = Open3.popen3('node',
      File.join(__dir__, 'tui_feedback_frames.mjs'), COLS.to_s, ROWS.to_s)
    @screens, @raw, @pending, @queries = [], ''.b, ''.b, ''.b
  end
  def now
    Process.clock_gettime(Process::CLOCK_MONOTONIC)
  end
  def write(text)
    @writer.write(text)
    @writer.flush
  end
  def type(text)
    write("\e[200~#{text}\e[201~")
    pump(0.15)
    wait('draft rendered before Enter') { |s| s['lines'].last(5).any? { |line| line.include?(text) } }
  end
  def pump(seconds)
    deadline = now + seconds
    while now < deadline
      next unless IO.select([@reader], nil, nil, [deadline - now, 0.01].min)
      data = @reader.read_nonblock(1_000_000, exception: false)
      raise 'fixture TUI exited unexpectedly' if data.nil?
      next unless data.is_a?(String)
      @raw << data
      @pending << data
      @queries << data
      @queries.gsub!(/\e\[(16t|5n|6n|c|>c|\?u)/) do
        write({'16t' => "\e[6;18;9t", '5n' => "\e[0n", '6n' => "\e[1;1R", 'c' => "\e[?1;2c",
          '>c' => "\e[>0;0;0c", '?u' => "\e[?0u"}.fetch(Regexp.last_match(1)))
        ''
      end
      @queries = @queries.byteslice(-64, 64) || @queries
      while (last = @pending.index("\e[?2026l"))
        frame = @pending.slice!(0, last + "\e[?2026l".bytesize)
        completed_at = now
        @decoder_in.puts(JSON.generate({bytes: Base64.strict_encode64(frame)}))
        @decoder_in.flush
        raise 'VT decoder response timeout' unless IO.select([@decoder_out], nil, nil, 3)
        screen = JSON.parse(@decoder_out.gets || raise('VT decoder exited'))
        @screens << screen.merge('at' => completed_at)
      end
    end
  rescue Errno::EIO
    raise 'fixture TUI exited unexpectedly (PTY EIO)'
  end
  def wait(label, seconds: 20)
    deadline = now + seconds
    loop do
      return @screens.last if @screens.last && yield(@screens.last)
      raise "#{label}: timed out" if now > deadline
      pump(0.02)
    end
  end
  def close
    Process.kill('TERM', @pid) rescue Errno::ESRCH
    # Close PTY endpoints before waiting: a terminal exit must not wait on our reader.
    @reader.close unless @reader.closed?
    @writer.close unless @writer.closed?
    begin
      Timeout.timeout(3) { Process.wait(@pid) }
    rescue Timeout::Error
      Process.kill('KILL', @pid) rescue Errno::ESRCH
      # Some macOS exiting processes cannot be reaped immediately; never block cleanup.
      Process.detach(@pid) rescue Errno::ECHILD
    rescue Errno::ECHILD
    end
    @reader.close unless @reader.closed?
    @writer.close unless @writer.closed?
    @decoder_in.close
    @decoder_wait.join(2)
    Process.kill('TERM', @decoder_wait.pid) if @decoder_wait.alive?
    @decoder_out.close
    @decoder_err.close
  end
end

def fixture_command(environment, *command)
  Open3.popen3(environment, *command, unsetenv_others: true, pgroup: true) do |input, output, error, process|
    input.close
    out = Thread.new { output.read }
    err = Thread.new { error.read }
    begin
      Timeout.timeout(35) do
        status = process.value
        raise "fixture command failed: #{command.last}: #{err.value}" unless status.success?
        out.value
      end
    rescue Timeout::Error
      Process.kill('TERM', -process.pid) rescue Errno::ESRCH
      raise 'fixture command timed out'
    ensure
      output.close unless output.closed?
      error.close unless error.closed?
      out.kill if out.alive?
      err.kill if err.alive?
    end
  end
end

binary, report_dir = File.realpath(ARGV.fetch(0)), File.expand_path(ARGV.fetch(1))
baseline = ARGV.include?('--expect-delayed')
FileUtils.mkdir_p(report_dir)
root = Dir.mktmpdir('wd-enter-feedback-')
home, workspace = File.join(root, 'user', '.willdeep'), File.join(root, 'workspace')
FileUtils.mkdir_p([home, workspace])
File.write(File.join(report_dir, 'fixture.json'), JSON.pretty_generate({isolated_root: root, binary: binary}))
environment = {'PATH' => ENV.fetch('PATH'), 'HOME' => File.join(root, 'user'), 'USERPROFILE' => File.join(root, 'user'),
  'WILLDEEP_HOME' => home, 'TERM' => 'xterm-256color', 'WILLDEEP_TELEMETRY_DISABLED' => '1', 'DO_NOT_TRACK' => '1',
  'WILLDEEP_LOCAL_DISCOVERY' => '0', 'LANG' => 'en_US.UTF-8', 'NO_PROXY' => '127.0.0.1,localhost', 'no_proxy' => '127.0.0.1,localhost',
  'XDG_CONFIG_HOME' => File.join(root, 'user', '.config'), 'XDG_CACHE_HOME' => File.join(root, 'user', '.cache'),
  'XDG_DATA_HOME' => File.join(root, 'user', '.local/share')}
provider = FeedbackHttpServer.new
config = File.join(home, 'config.toml')
config_lines = ['version = 1', "default_provider = 'alpha'", '[agent]', 'max_turns = 2', 'auto_title = false',
  'input_suggestions = false', 'small_model_routing = false', 'auto_dispatch_read_only = false', "approval = 'smart'",
  '[ui]', "mode = 'compact'", 'check_updates = false']
%w[alpha beta].each do |profile|
  config_lines += ["[providers.#{profile}]", "provider = 'openai-compatible'", "api = 'chat-completions'",
    "api_base = 'http://127.0.0.1:#{provider.port}/#{profile}/v1'", "api_key = 'fixture-only'", "model = '#{profile}-a'"]
end
File.write(config, config_lines.join("\n") + "\n", perm: 0600)
results, errors = [], []
state_path = File.join(home, 'runtime', 'daemon.json')
begin
  version = fixture_command(environment, binary, '--version').strip
  fixture_command(environment, binary, '--config', config, 'daemon', 'start')
  # This state belongs exclusively to the process started above in the temporary HOME.
  # Its generated authentication nonce is used in memory and never included in reports.
  original_state = JSON.parse(File.read(state_path))
  host, port = original_state.fetch('address').split(':')
  gate = FeedbackGate.new
  proxy = FeedbackHttpServer.new(gate: gate, upstream: [host, Integer(port)])
  File.write(state_path, JSON.generate(original_state.merge('address' => "127.0.0.1:#{proxy.port}", 'local_transport' => nil)), perm: 0600)
  tui = FeedbackPty.new(binary, environment, config, workspace)
  tui.wait('initial terminal frame') { |s| s['text'].include?('Focus') }
  measure = lambda do |name, operation, matcher, failure: false, key: "\r"|
    gate.arm(operation, failure: failure)
    first = tui.screens.length
    started = tui.now
    tui.write(key)
    deadline = started + 2.0
    tui.pump(0.02) while tui.now < deadline
    entered_before_release = gate.entered?
    released_at = tui.now
    gate.release
    tui.wait("#{name} visible feedback", seconds: 12) do |_screen|
      tui.screens.drop(first).any? { |s| matcher.call(s) }
    end
    feedback = tui.screens.drop(first).find { |s| matcher.call(s) }
    delay = ((feedback.fetch('at') - started) * 1000).round(2)
    passed = entered_before_release && feedback.fetch('at') < released_at && delay <= 300
    passed = entered_before_release && feedback.fetch('at') >= released_at && delay >= 1900 if baseline
    row = {name: name, passed: passed, feedback_ms: delay, threshold_ms: 300,
      gate_operation: operation, gate_entered_before_release: entered_before_release,
      feedback_before_release: feedback.fetch('at') < released_at, gate_hold_ms: ((released_at - started) * 1000).round(2),
      expected_failure: failure}
    results << row
    File.write(File.join(report_dir, 'measurements.json'), JSON.pretty_generate(results))
    File.write(File.join(report_dir, "#{name}.screen.json"), JSON.pretty_generate(feedback))
    raise "#{name}: feedback #{delay} ms, gate entered=#{entered_before_release}" unless passed
    row
  ensure
    gate.release
  end
  selected = lambda do |target|
    lambda do |screen|
      !screen['text'].include?('Select model') && screen['lines'].first(FeedbackPty::ROWS - 5).any? do |line|
        line.include?(target) && line.match?(/switch|select/i)
      end
    end
  end
  open_picker = lambda do |query, profile = nil, current = 'alpha'|
    tui.type('/model')
    tui.write("\r")
    tui.wait('model list') { |s| s['text'].include?('Select model') && s['text'].include?("#{current}-a") && !s['text'].include?('Refreshing models') }
    if profile == 'beta'
      tui.write("\t\e[B\t")
      tui.wait('beta model list') { |s| s['text'].include?('beta-a') && !s['text'].include?('Refreshing models') }
    end
    tui.write(query)
    tui.pump(0.2)
  end
  model_in_store = lambda do |expected_profile, expected_model|
    sessions = Dir[File.join(home, 'sessions', '*.json')].map { |path| JSON.parse(File.read(path)) }
    !sessions.empty? && sessions.all? { |s| s['profile'] == expected_profile && s['model'] == expected_model }
  end
  open_picker.call('alpha-b')
  measure.call('same_provider_selection', 'session.update_model', selected.call('alpha-b'))
  # Repeated empty confirmation occurs only after measuring feedback from one Enter.
  tui.write("\r\r") unless baseline
  tui.wait('same Provider committed') { |_s| model_in_store.call('alpha', 'alpha-b') }
  unless baseline
    tui.wait('same Provider success frame') { |s| s['text'].match?(/switched.*alpha-b/i) }
    # Failed control operation must still close immediately, then visibly report failure.
    open_picker.call('alpha-c')
    measure.call('model_failure', 'session.update_model', selected.call('alpha-c'), failure: true)
    tui.wait('model failure frame') { |s| s['text'].match?(/switch failed/i) }
    raise 'failed switch changed persisted model' unless model_in_store.call('alpha', 'alpha-b')
    tui.type('/model alpha-a')
    measure.call('direct_model_command', 'session.update_model', selected.call('alpha-a'))
    tui.wait('direct switch committed') { |_s| model_in_store.call('alpha', 'alpha-a') }
    open_picker.call('beta-c', 'beta')
    measure.call('cross_provider_selection', 'session.configure', selected.call('beta-c'))
    tui.wait('cross Provider committed') { |_s| model_in_store.call('beta', 'beta-c') }
    open_picker.call('beta-b', nil, 'beta')
    line_index = tui.screens.last['lines'].each_index.select { |i| tui.screens.last['lines'][i].include?('beta-b') }.last
    column = tui.screens.last['lines'][line_index].index('beta-b')
    click = "\e[<0;#{column + 1};#{line_index + 1}M\e[<0;#{column + 1};#{line_index + 1}m"
    measure.call('mouse_model_selection', 'session.update_model', selected.call('beta-b'), key: click)
    tui.wait('mouse selection committed') { |_s| model_in_store.call('beta', 'beta-b') }
    tui.wait('mouse selection completed frame') { |s| s['text'].match?(/switched.*beta-b/i) }
    raise 'duplicate Enter submitted a turn' if proxy.requests.any? { |r| r[:operation] == 'turn.submit' }
    prompt = 'latency-normal-001'
    tui.type(prompt)
    user_feedback = ->(s) { s['lines'].first(FeedbackPty::ROWS - 5).any? { |line| line.include?(prompt) } && s['lines'].last(5).none? { |line| line.include?(prompt) } }
    measure.call('ordinary_message', 'session.create', user_feedback)
    tui.wait('ordinary turn completed') { |s| s['text'].include?('FIXTURE_FEEDBACK_REPLY') && s['lines'].last.start_with?('Ready') }
    # Start a long-lived model response, then submit another message while busy.
    previous_posts = provider.requests.count { |r| r[:method] == 'POST' }
    provider.hold_provider = true
    tui.type('hold-open-for-busy-feedback')
    tui.write("\r")
    tui.wait('busy Runtime turn') { |_s| provider.requests.count { |r| r[:method] == 'POST' } > previous_posts }
    busy_prompt = 'latency-busy-001'
    tui.type(busy_prompt)
    busy_feedback = ->(s) { s['lines'].first(FeedbackPty::ROWS - 5).any? { |line| line.include?(busy_prompt) } && s['lines'].last(5).none? { |line| line.include?(busy_prompt) } }
    measure.call('busy_message', 'session.create', busy_feedback)
    tui.pump(0.3)
    rejected_prompt = 'latency-busy-rejected-001'
    before_rejected = proxy.requests.count { |r| %w[turn.submit turn.steer].include?(r[:operation]) }
    tui.type(rejected_prompt)
    rejected_feedback = ->(s) { s['lines'].first(FeedbackPty::ROWS - 5).any? { |line| line.include?(rejected_prompt) } && s['lines'].last(5).none? { |line| line.include?(rejected_prompt) } }
    measure.call('busy_submission_failure', 'session.create', rejected_feedback, failure: true)
    tui.wait('failed busy draft restored') do |s|
      s['text'].include?('Submission failed') && s['lines'].last(5).any? { |line| line.include?(rejected_prompt) }
    end
    after_rejected = proxy.requests.count { |r| %w[turn.submit turn.steer].include?(r[:operation]) }
    raise 'rejected busy submission reached Runtime dispatch' unless before_rejected == after_rejected
    provider.hold_provider = false
    tui.pump(0.5)
  end
rescue StandardError => error
  errors << "#{error.class}: #{error.message}"
ensure
  gate&.release
  provider.hold_provider = false
  File.binwrite(File.join(report_dir, 'terminal.ansi'), tui.raw) if tui
  tui&.close
  # Restore only this fixture's state so daemon stop bypasses the test proxy.
  File.write(state_path, JSON.generate(original_state), perm: 0600) if original_state
  begin
    fixture_command(environment, binary, 'daemon', 'stop') if original_state
  rescue StandardError => error
    errors << "cleanup: #{error.message}"
  end
  proxy&.close
  provider.close
  report = {passed: errors.empty? && !results.empty? && results.all? { |r| r[:passed] },
    binary: binary, version: version, baseline_expected_delayed: baseline, isolated_root: root,
    results: results, errors: errors, runtime_operations: proxy&.requests&.map { |r| r[:operation] }&.compact}
  File.write(File.join(report_dir, 'report.json'), JSON.pretty_generate(report))
  markdown = "# Enter 即时反馈 PTY 验收\n\n版本：#{version}\n\n整体结果：#{report[:passed] ? 'PASS' : 'FAIL'}\n\n"
  markdown += results.map { |r| "- #{r[:name]}: #{r[:passed] ? 'PASS' : 'FAIL'}，首帧 #{r[:feedback_ms]} ms；控制请求阻塞 #{r[:gate_hold_ms]} ms。" }.join("\n")
  markdown += "\n\n" + errors.map { |e| "- #{e}" }.join("\n") unless errors.empty?
  File.write(File.join(report_dir, 'report.md'), markdown + "\n")
  puts JSON.generate(report)
end
exit(report[:passed] ? 0 : 1)
