#!/usr/bin/env ruby
# Isolated processes and a deterministic loopback fixture. No live model accounts.
require 'json'
require 'socket'
require 'open3'
require 'timeout'
require 'tmpdir'
require 'fileutils'
require 'securerandom'
require 'optparse'
require 'rbconfig'
require 'digest'
require 'net/http'

class MagpieAgentFixture
  attr_reader :port

  def initialize(redirect: false, redirect_target: nil)
    @listener = TCPServer.new('127.0.0.1', 0)
    @port = @listener.addr[1]
    @redirect = redirect
    @redirect_target = redirect_target || 'http://127.0.0.1:1/redirect-target'
    @requests = []
    @mutex = Mutex.new
    @workers = []
    @acceptor = Thread.new do
      loop do
        socket = @listener.accept
        @workers << Thread.new(socket) do |client|
          begin
            Timeout.timeout(20) { respond(client) }
          rescue EOFError, IOError, Errno::ECONNRESET, Errno::EPIPE, Timeout::Error
            # A cancelled request can close its fixture socket before the reply.
          ensure
            client.close
          end
        end
      end
    rescue IOError, Errno::EBADF
      # Fixture shutdown closes the listener.
    end
  end

  def requests
    @mutex.synchronize { @requests.dup }
  end

  def base_url
    "http://127.0.0.1:#{port}/v1"
  end

  def close
    @listener.close
    @acceptor.join
    @workers.each(&:join)
  end

  private

  def respond(client)
    request_line = client.gets or return
    method, path = request_line.split
    headers = {}
    while (line = client.gets) && line != "\r\n"
      name, value = line.split(':', 2)
      headers[name.downcase] = value.to_s.strip
    end
    text = client.read(Integer(headers.fetch('content-length', '0')))
    body = text.empty? ? {} : JSON.parse(text)
    @mutex.synchronize do
      @requests << { method: method, path: path, model: body['model'], marker: headers['authorization'] }
    end
    if @redirect
      response(client, 307, 'text/plain', 'redirect forbidden', "Location: #{@redirect_target}\r\n")
    elsif method == 'GET'
      models = %w[mock/model-a mock/model-b native/model worker/model].map do |id|
        { id: id, object: 'model', owned_by: 'fixture' }
      end
      response(client, 200, 'application/json', JSON.generate(object: 'list', data: models))
    else
      reply = { id: 'chat_magpie_fixture', object: 'chat.completion', model: body['model'],
                choices: [{ index: 0, message: { role: 'assistant', content: 'magpie-agent-ok' }, finish_reason: 'stop' }],
                usage: { prompt_tokens: 10, completion_tokens: 4, total_tokens: 14 } }
      if body['stream']
        chunks = [
          { id: reply[:id], object: 'chat.completion.chunk', model: body['model'], choices: [{ index: 0, delta: { role: 'assistant', content: 'magpie-agent-ok' }, finish_reason: nil }] },
          { id: reply[:id], object: 'chat.completion.chunk', model: body['model'], choices: [{ index: 0, delta: {}, finish_reason: 'stop' }], usage: reply[:usage] }
        ].map { |chunk| "data: #{JSON.generate(chunk)}\n\n" }.join + "data: [DONE]\n\n"
        response(client, 200, 'text/event-stream', chunks)
      else
        response(client, 200, 'application/json', JSON.generate(reply))
      end
    end
  end

  def response(client, code, content_type, body, extra = '')
    reason = code == 200 ? 'OK' : 'Temporary Redirect'
    client.write("HTTP/1.1 #{code} #{reason}\r\nContent-Type: #{content_type}\r\nContent-Length: #{body.bytesize}\r\n#{extra}Connection: close\r\n\r\n#{body}")
  end
end

class MagpieAgentRuntimeVerification
  def initialize(binary, report_dir, magpie_binary: nil)
    @binary = File.expand_path(binary)
    @magpie_binary = magpie_binary && File.realpath(magpie_binary)
    @report_dir = File.expand_path(report_dir)
    @checks = []
    @homes = []
  end

  def run
    @fixture = MagpieAgentFixture.new
    Dir.mktmpdir('wd-magpie-agents-', '/private/tmp') do |root|
      @root = root
      @env = isolated_environment('runtime')
      @home = @env.fetch('WILLDEEP_HOME')
      @version = command(@env, '--version').strip
      @binary_digest = Digest::SHA256.file(@binary).hexdigest
      @workspace = File.join(root, 'workspace')
      FileUtils.mkdir_p(@workspace)
      @config = File.join(@home, 'config.toml')
      write_config(@config)
      @original_config = File.binread(@config)
      @connection = File.join(@home, 'integrations/magpie/cli/connection.json')
      FileUtils.mkdir_p(File.dirname(@connection))
      begin
        if @magpie_binary
          return unless check('real_magpie_gateway_and_local_packages') { start_magpie }
        end
        check('runtime_started_before_connection') do
          command(@env, 'daemon', 'start')
          raise 'connection existed before Runtime startup' if File.exist?(@connection)
          @runtime_status = command(@env, 'daemon', 'status').strip
          raise 'Runtime did not report running' unless @runtime_status.start_with?("running\t")
        end
        check('unconnected_uses_native_default') do
          @native = create_session
          turn(@native, 'native before connection')
          assert_last_request('native/model', 'mock-native')
        end
        check('connected_new_session_uses_a') do
          connect('mock/model-a')
          @session_a = create_session
          raise 'session did not pin the integration profile' unless @session_a['profile'] == 'magpie-cli'
          raise 'session did not pin model A' unless @session_a['model'] == 'mock/model-a'
          turn(@session_a, 'connected A')
          assert_last_request('mock/model-a', 'magpie-willdeep-cli')
        end
        check('client_generated_new_session_id_pins_integration') do
          session = create_session('id' => SecureRandom.uuid)
          raise 'new client ID bypassed integration default' unless session['profile'] == 'magpie-cli' && session['model'] == 'mock/model-a'
          turn(session, 'new client-generated session ID')
          assert_last_request('mock/model-a', 'magpie-willdeep-cli')
        end
        check('real_tui_implicit_magpie_wide_and_narrow') do
          fixture_path = File.join(@root, 'pty-fixture.json')
          File.write(fixture_path, JSON.generate(root: @root, environment: @env, workspace: @workspace))
          count = post_requests.length
          result = command_result(@env, '-EUTF-8', File.join(__dir__, 'magpie_agents_pty.rb'),
            '--fixture', fixture_path, '--binary', @binary, '--report-dir', @report_dir,
            binary: RbConfig.ruby, timeout_seconds: 150)
          raise "PTY verification: #{result[:stderr]} #{result[:stdout]}" unless result[:success]
          raise 'both terminal sizes must call the fixture model' unless post_requests.length >= count + 2
          assert_last_request('mock/model-a', 'magpie-willdeep-cli')
        end
        check('model_change_keeps_existing_a') do
          connect('mock/model-b')
          turn(@session_a, 'existing A after default changed')
          assert_last_request('mock/model-a', 'magpie-willdeep-cli')
        end
        check('model_change_new_session_uses_b') do
          @session_b = create_session
          turn(@session_b, 'new B')
          assert_last_request('mock/model-b', 'magpie-willdeep-cli')
        end
        check('preconnection_session_keeps_native_provider') do
          turn(@native, 'existing native while connected')
          assert_last_request('native/model', 'mock-native')
        end
        check('explicit_native_profile_wins') do
          session = create_session('profile' => 'native')
          turn(session, 'explicit native')
          assert_last_request('native/model', 'mock-native')
        end
        check('disabled_restores_native_default') do
          if @magpie_binary
            magpie_command('willdeep-cli', 'default')
          else
            File.write(@connection, JSON.generate(enabled: false))
          end
          session = create_session
          turn(session, 'native after disabled')
          assert_last_request('native/model', 'mock-native')
        end
        check('disconnected_pinned_session_does_not_fallback') do
          count = post_requests.length
          result = turn(@session_a, 'do not fallback', expected: 'failed')
          raise 'disconnected session reached a model' unless post_requests.length == count
          raise 'missing failed turn evidence' unless result['status'] == 'failed'
        end
        check('native_model_only_restores_default') do
          if @magpie_binary
            connect('mock/model-b')
            magpie_command('willdeep-cli', 'restored-native-model')
            raise 'host did not leave a model-only object' unless JSON.parse(File.read(@connection)) == { 'model' => 'restored-native-model' }
          else
            File.write(@connection, JSON.generate(model: 'restored-native-model'))
          end
          session = create_session
          turn(session, 'restored original native provider')
          assert_last_request('native/model', 'mock-native')
        end
        check('invalid_connection_blocks_implicit_session') do
          File.write(@connection, '{"enabled":true,"model":"incomplete"}')
          count = post_requests.length
          result = api_result('session.create', 'workspace' => @workspace)
          raise 'invalid integration was accepted' if result.fetch('success')
          raise 'invalid integration reached a model' unless post_requests.length == count
        end
        check('invalid_connection_allows_explicit_native') do
          session = create_session('profile' => 'native')
          turn(session, 'explicit native survives invalid integration')
          assert_last_request('native/model', 'mock-native')
        end
        check('custom_config_bypasses_implicit_integration') do
          custom = File.join(@root, 'custom.toml')
          write_config(custom)
          command(@env, 'run', '--local', '--config', custom, '--workspace', @workspace,
                  '--output', 'ndjson', 'explicit custom configuration')
          assert_last_request('native/model', 'mock-native')
        end
        check('explicit_model_bypasses_implicit_integration') do
          connect('mock/model-b')
          command(@env, 'run', '--local', '--model', 'native/model', '--workspace', @workspace,
                  '--output', 'ndjson', 'explicit model selection')
          assert_last_request('native/model', 'mock-native')
        end
        check('integration_rejects_redirects') do
          redirect = MagpieAgentFixture.new(redirect: true, redirect_target: "#{@fixture.base_url}/chat/completions")
          begin
            redirect_env = isolated_environment('redirect')
            write_config(File.join(redirect_env.fetch('WILLDEEP_HOME'), 'config.toml'))
            path = File.join(redirect_env.fetch('WILLDEEP_HOME'), 'integrations/magpie/cli/connection.json')
            FileUtils.mkdir_p(File.dirname(path))
            File.write(path, JSON.generate(connection('mock/model-a').merge(base_url: redirect.base_url)))
            count = post_requests.length
            result = command_result(redirect_env, 'run', '--local', '--workspace', @workspace,
                                    '--output', 'ndjson', 'reject redirect')
            raise 'redirecting integration succeeded' if result[:success]
            raise 'redirect target received a model request' unless post_requests.length == count
            raise 'request never reached redirect fixture' if redirect.requests.empty?
          ensure
            redirect.close
          end
        end
        check('toml_configuration_unchanged') do
          current_config = File.binread(@config)
          FileUtils.mkdir_p(@report_dir)
          { 'before' => @original_config, 'after' => current_config }.each do |stage, contents|
            File.binwrite(File.join(@report_dir, "config.#{stage}.toml"), contents)
          end
          raise 'integration rewrote config.toml; isolated before/after evidence retained' unless current_config == @original_config
        end
        check('custom_home_does_not_fall_back_to_standard_home') do
          connect('mock/model-a')
          alternative = @env.merge('WILLDEEP_HOME' => File.join(@root, 'alternative-willdeep'))
          FileUtils.mkdir_p(alternative.fetch('WILLDEEP_HOME'))
          write_config(File.join(alternative.fetch('WILLDEEP_HOME'), 'config.toml'))
          command(alternative, 'run', '--local', '--workspace', @workspace, '--output', 'ndjson', 'custom root')
          assert_last_request('native/model', 'mock-native')
        end
        check('session_source_survives_same_named_user_profile') do
          environment = isolated_environment('collision')
          config_path = File.join(environment.fetch('WILLDEEP_HOME'), 'config.toml')
          write_config(config_path)
          path = File.join(environment.fetch('WILLDEEP_HOME'), 'integrations/magpie/cli/connection.json')
          FileUtils.mkdir_p(File.dirname(path))
          File.write(path, JSON.generate(connection('mock/model-a')))
          first = JSON.parse(command(environment, 'run', '--local', '--workspace', @workspace,
                                     '--output', 'json', 'pin integration source'))
          session_id = first.fetch('session_id')
          File.write(path, '{}')
          user_profile = "\n[providers.magpie-cli]\nprovider='openai-compatible'\napi='chat-completions'\napi_base='#{@fixture.base_url}'\napi_key='mock-native'\nmodel='native/model'\n"
          File.write(config_path, File.read(config_path) + user_profile)
          count = post_requests.length
          resumed = command_result(environment, 'run', '--local', '--workspace', @workspace,
                                   '--session', session_id, '--output', 'json', 'no source substitution')
          raise 'disconnected integration fell back to same-named user profile' if resumed[:success]
          raise 'resumed integration reached a model after disconnect' unless post_requests.length == count
          command(environment, 'run', '--local', '--workspace', @workspace, '--profile', 'magpie-cli',
                  '--output', 'json', 'explicit original user profile')
          assert_last_request('native/model', 'mock-native')
        end
        check('integration_ignores_environment_proxies') do
          proxy = MagpieAgentFixture.new(redirect: true)
          begin
            proxy_env = isolated_environment('proxy')
            write_config(File.join(proxy_env.fetch('WILLDEEP_HOME'), 'config.toml'))
            path = File.join(proxy_env.fetch('WILLDEEP_HOME'), 'integrations/magpie/cli/connection.json')
            FileUtils.mkdir_p(File.dirname(path))
            File.write(path, JSON.generate(connection('mock/model-a')))
            %w[HTTP_PROXY HTTPS_PROXY ALL_PROXY http_proxy https_proxy all_proxy].each do |key|
              proxy_env[key] = "http://127.0.0.1:#{proxy.port}"
            end
            proxy_env['NO_PROXY'] = proxy_env['no_proxy'] = ''
            command(proxy_env, 'run', '--local', '--workspace', @workspace, '--output', 'ndjson', 'proxy bypass')
            assert_last_request('mock/model-a', 'magpie-willdeep-cli')
            raise 'integration used the configured proxy' unless proxy.requests.empty?
          ensure
            proxy.close
          end
        end
      ensure
        @homes.each do |environment|
          command(environment, 'daemon', 'stop')
        rescue StandardError => error
          @checks << { name: 'runtime_cleanup', status: 'failed', detail: error.message }
        end
        begin
          stop_magpie
        rescue StandardError => error
          @checks << { name: 'magpie_cleanup', status: 'failed', detail: error.message }
        end
      end
    end
  ensure
    @fixture&.close
    write_report
  end

  private

  def isolated_environment(name)
    home = File.join(@root, name)
    config_home = File.join(home, '.willdeep')
    FileUtils.mkdir_p(config_home)
    environment = { 'PATH' => ENV.fetch('PATH'), 'HOME' => home, 'USERPROFILE' => home,
                    'WILLDEEP_HOME' => config_home, 'WILLDEEP_LANGUAGE' => 'en',
                    'XDG_CONFIG_HOME' => File.join(home, '.config'), 'XDG_DATA_HOME' => File.join(home, '.local/share'),
                    'XDG_CACHE_HOME' => File.join(home, '.cache'), 'DO_NOT_TRACK' => '1', 'WILLDEEP_TELEMETRY_DISABLED' => '1',
                    'TERM' => 'xterm-256color', 'LANG' => 'en_US.UTF-8', 'WILLDEEP_LOCAL_DISCOVERY' => '0' }
    @homes << environment
    environment
  end

  def write_config(path)
    File.write(path, "version=1\ndefault_provider='native'\n[agent]\nauto_title=false\nmax_turns=2\napproval='smart'\n[providers.native]\nprovider='openai-compatible'\napi='chat-completions'\napi_base='#{@fixture.base_url}'\napi_key='mock-native'\nmodel='native/model'\n")
    File.chmod(0o600, path)
  end

  def connection(model)
    { schema_version: 1, client_id: 'willdeep-cli', enabled: true, base_url: @fixture.base_url,
      client_marker: 'magpie-willdeep-cli', model: model, context_window: 32768, max_output_tokens: 4096 }.merge(base_url: @magpie_base || @fixture.base_url)
  end

  def connect(model)
    if @magpie_binary
      magpie_command('willdeep-cli', "magpie/#{model}")
      value = JSON.parse(File.read(@connection))
      unless value['model'] == model && value['base_url'] == @magpie_base && value['client_marker'] == 'magpie-willdeep-cli'
        raise 'real host wrote an unexpected CLI connection'
      end
    else
      File.write(@connection, JSON.generate(connection(model)))
    end
  end

  def start_magpie
    listener = TCPServer.new('127.0.0.1', 0)
    port = listener.addr[1]
    listener.close
    @magpie_base = "http://127.0.0.1:#{port}/v1"
    bun = ENV['MAGPIE_BUN'] || ENV.fetch('PATH').split(File::PATH_SEPARATOR)
      .map { |directory| File.join(directory, 'bun') }.find { |path| File.file?(path) && File.executable?(path) }
    raise 'real Magpie verification requires an existing Bun executable; no runtime is installed' unless bun && File.executable?(bun)
    @magpie_environment = @env.merge(
      'MAGPIE_ADDR' => "127.0.0.1:#{port}", 'MAGPIE_NO_ACCOUNTS' => '1',
      'MAGPIE_PARTNERS' => 'off', 'MAGPIE_BUN' => File.realpath(bun),
      'MAGPIE_PLUGIN_MARKET' => "http://127.0.0.1:#{@fixture.port}/market.json",
      'HTTP_PROXY' => 'http://127.0.0.1:1', 'HTTPS_PROXY' => 'http://127.0.0.1:1',
      'ALL_PROXY' => 'http://127.0.0.1:1', 'NO_PROXY' => '127.0.0.1,localhost,::1',
      'NO_COLOR' => '1'
    )
    @magpie_version = magpie_command('version').strip
    packages = File.expand_path('../../integrations/magpie', __dir__)
    %w[agent-willdeep agent-willdeep-cli].each do |name|
      magpie_command('plugin', 'add', File.join(packages, name))
    end
    magpie_command('provider', 'add', 'Mock', 'id=mock', "url=#{@fixture.base_url}",
                   'key=mock-upstream', 'models=model-a,model-b')
    FileUtils.mkdir_p(@report_dir)
    @magpie_log = File.open(File.join(@report_dir, 'magpie-gateway.log'), 'w')
    @magpie_pid = Process.spawn(@magpie_environment, @magpie_binary, 'serve',
      unsetenv_others: true, pgroup: true, out: @magpie_log, err: @magpie_log)
    Timeout.timeout(20) do
      loop do
        begin
          response = Net::HTTP.start('127.0.0.1', port, nil, open_timeout: 1, read_timeout: 1) do |http|
            http.get('/v1/models', 'Authorization' => 'Bearer magpie-willdeep-cli')
          end
          catalog = JSON.parse(response.body) if response.code == '200'
          break if catalog && catalog.fetch('data').any? { |model| model['id'] == 'mock/model-a' }
        rescue Errno::ECONNREFUSED, Net::OpenTimeout, Net::ReadTimeout
          # The isolated gateway has not bound its listener yet.
        end
        sleep 0.1
      end
    end
  end

  def magpie_command(*arguments)
    result = command_result(@magpie_environment, *arguments, binary: @magpie_binary)
    raise "Magpie #{arguments.first}: exit #{result[:exit_code]}: #{result[:stderr][0, 700]}" unless result[:success]
    result[:stdout]
  end

  def stop_magpie
    return unless @magpie_pid
    unless Process.waitpid(@magpie_pid, Process::WNOHANG)
      Process.kill('TERM', -@magpie_pid)
      begin
        Timeout.timeout(5) { Process.waitpid(@magpie_pid) }
      rescue Timeout::Error
        Process.kill('KILL', -@magpie_pid)
        Process.waitpid(@magpie_pid)
      end
    end
  ensure
    @magpie_log&.close
    @magpie_pid = nil
  end

  def command_result(environment, *arguments, input: nil, binary: @binary, timeout_seconds: 60)
    Open3.popen3(environment, binary, *arguments, unsetenv_others: true, pgroup: true) do |stdin, stdout, stderr, child|
      readers = [Thread.new { stdout.read }, Thread.new { stderr.read }]
      begin
        Timeout.timeout(timeout_seconds) do
          stdin.write(input) if input
          stdin.close
          status = child.value
          { success: status.success?, stdout: readers[0].value, stderr: readers[1].value, exit_code: status.exitstatus }
        end
      ensure
        stdin.close unless stdin.closed?
        if child.alive? || readers.any?(&:alive?)
          begin
            Process.kill('TERM', -child.pid)
            child.join(3)
            readers.each { |reader| reader.join(1) }
            Process.kill('KILL', -child.pid) if child.alive? || readers.any?(&:alive?)
          rescue Errno::ESRCH
            # The owned process group has already exited.
          end
        end
        child.join(2)
        readers.each do |reader|
          reader.join(1)
          reader.kill if reader.alive?
        end
      end
    end
  end

  def command(environment, *arguments)
    count = post_requests.length if arguments.first == 'run'
    result = command_result(environment, *arguments)
    raise "#{arguments.first}: exit #{result[:exit_code]}: #{result[:stderr][0, 700]}" unless result[:success]
    if count && (post_requests.length <= count || !result[:stdout].include?('magpie-agent-ok'))
      raise 'local run did not produce a fresh fixture model request and reply'
    end
    result[:stdout]
  end

  def api_result(operation, params)
    result = command_result(@env, 'api', operation, '--params-file', '-', input: JSON.generate(params))
    parsed = begin
      JSON.parse(result[:stdout])
    rescue JSON::ParserError
      nil
    end
    { 'success' => result[:success] && parsed.is_a?(Hash) && parsed['status'] == 'ok',
      'reply' => parsed, 'stderr' => result[:stderr] }
  end

  def api(operation, params)
    result = api_result(operation, params)
    raise "#{operation}: #{result['stderr'][0, 700]}" unless result['success']
    result.fetch('reply').fetch('data')
  end

  def create_session(overrides = {})
    api('session.create', { 'workspace' => @workspace }.merge(overrides))
  end

  def turn(session, prompt, expected: 'completed')
    raise 'prerequisite session was not created' unless session
    request_count = post_requests.length
    submission = api_result('turn.submit', 'session_id' => session.fetch('id'), 'turn_request_id' => SecureRandom.uuid, 'prompt' => prompt)
    unless submission['success']
      if expected == 'failed' && submission.to_s.match?(/magpie/i)
        return { 'status' => 'failed', 'rejected_before_queue' => true }
      end
      raise "turn.submit: #{submission['stderr'][0, 700]}"
    end
    result = submission.fetch('reply').fetch('data')
    Timeout.timeout(40) do
      loop do
        result = api('turn.get', 'id' => result.fetch('id'))
        if %w[completed failed cancelled interrupted partial].include?(result['status'])
          raise "turn status #{result['status']}, expected #{expected}" unless result['status'] == expected
          if expected == 'completed' && post_requests.length <= request_count
            raise 'completed turn had no request to the fixture model'
          end
          return result
        end
        sleep 0.1
      end
    end
  end

  def post_requests
    @fixture.requests.select { |request| request[:method] == 'POST' }
  end

  def assert_last_request(model, marker)
    request = post_requests.last
    raise 'fixture did not receive a model request' unless request
    if @magpie_binary && marker == 'magpie-willdeep-cli' && request[:marker] == 'Bearer mock-upstream'
      model = model.delete_prefix('mock/')
      marker = 'mock-upstream'
    end
    raise "expected model #{model}, got #{request[:model]}" unless request[:model] == model
    raise 'incorrect client identity' unless request[:marker] == "Bearer #{marker}"
  end

  def check(name)
    yield
    @checks << { name: name, status: 'passed' }
    puts "PASS #{name}"
    true
  rescue StandardError => error
    @checks << { name: name, status: 'failed', detail: error.message }
    warn "FAIL #{name}: #{error.message}"
    false
  end

  def write_report
    FileUtils.mkdir_p(@report_dir)
    passed = @checks.any? && @checks.all? { |check| check[:status] == 'passed' }
    report = { passed: passed, version: @version, binary_sha256: @binary_digest,
               runtime_started_before_connection: @checks.any? { |check| check[:name] == 'runtime_started_before_connection' && check[:status] == 'passed' },
               paid_requests: 0, live_credentials_used: false,
               magpie_version: @magpie_version, real_magpie: !@magpie_binary.nil?,
               fixture: @magpie_binary ? 'real Magpie gateway and Agent plugins, deterministic local upstream' : 'deterministic loopback OpenAI-compatible gateway',
               scope: 'CLI/Runtime consumption and real PTY; Desktop and full host registration matrix are verified separately',
               checks: @checks }
    File.write(File.join(@report_dir, 'runtime.json'), JSON.pretty_generate(report) + "\n")
    rows = @checks.map { |check| "| #{check[:name]} | #{check[:status]} | #{check.fetch(:detail, '').gsub('|', '\\|').gsub(/\s+/, ' ')} |" }
    text = ["# Magpie Agent CLI/Runtime 验收", '', "结果：#{passed ? '通过' : '失败'}；付费请求：0；未使用真实账号。", '',
            '范围：隔离 CLI/Runtime 消费链。真实 Magpie 插件注册及 Desktop 验收单独记录。', '',
            '| 场景 | 结果 | 详情 |', '| --- | --- | --- |', *rows].join("\n") + "\n"
    File.write(File.join(@report_dir, 'runtime.md'), text)
    puts JSON.generate(report)
  end
end

if $PROGRAM_NAME == __FILE__
  options = { binary: ENV.fetch('WILLDEEP_TEST_BIN', 'target/debug/willdeep'), magpie: ENV['MAGPIE_TEST_BIN'], report: 'build/verification/magpie-agents' }
  OptionParser.new do |parser|
    parser.on('--binary PATH') { |value| options[:binary] = value }
    parser.on('--report-dir PATH') { |value| options[:report] = value }
    parser.on('--magpie-binary PATH') { |value| options[:magpie] = value }
  end.parse!
  verification = MagpieAgentRuntimeVerification.new(options[:binary], options[:report], magpie_binary: options[:magpie])
  verification.run
  report = JSON.parse(File.read(File.join(options[:report], 'runtime.json')))
  exit(report.fetch('passed') ? 0 : 1)
end
