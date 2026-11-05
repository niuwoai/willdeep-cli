#!/usr/bin/env ruby
# frozen_string_literal: true

# Real CLI -> isolated Runtime -> deterministic loopback Provider -> built-in
# configuration tools. Never uses the user's configuration, daemon or credentials.
Encoding.default_external = Encoding::UTF_8

require 'json'
require 'socket'
require 'open3'
require 'timeout'
require 'tmpdir'
require 'fileutils'

class ConfigChatProvider
  attr_reader :models, :outputs, :tool_names

  def initialize
    @listener = TCPServer.new('127.0.0.1', 0)
    @port = @listener.addr[1]
    @workers = []
    reset(:edit)
    @acceptor = Thread.new do
      loop do
        client = @listener.accept
        @workers << Thread.new(client) do |socket|
          Timeout.timeout(30) { respond(socket) }
        rescue EOFError, IOError, Errno::ECONNRESET, Errno::EPIPE
          # The tested process may cancel an auxiliary response.
        ensure
          socket.close
        end
      end
    rescue IOError, Errno::EBADF
      # Closing the listener ends the fixture.
    end
  end

  def base_url = "http://127.0.0.1:#{@port}/v1"

  def reset(mode)
    @mode = mode
    @step = 0
    @models = []
    @outputs = []
    @tool_names = []
  end

  def close
    @listener.close
    @acceptor.join
    @workers.each(&:join)
  end

  private

  def respond(socket)
    request = socket.gets or return
    method = request.split.first
    headers = {}
    while (line = socket.gets) && line != "\r\n"
      key, value = line.split(':', 2)
      headers[key.downcase] = value.to_s.strip
    end
    if method == 'GET'
      models = %w[main-fixture summary-old summary-new].map { |id| { id: id, object: 'model', owned_by: 'fixture' } }
      response(socket, 'application/json', JSON.generate(object: 'list', data: models))
      return
    end
    body = JSON.parse(socket.read(Integer(headers.fetch('content-length', '0'))))
    @models << body['model']
    @tool_names |= Array(body['tools']).map { |tool| tool.dig('function', 'name') }.compact
    @outputs = Array(body['messages']).select { |message| message['role'] == 'tool' }.map { |message| message['content'].to_s }
    message = if body['model'] != 'main-fixture'
                { role: 'assistant', content: 'Configuration task summary: retain the user request and continue safely.' }
              elsif @mode == :check
                { role: 'assistant', content: 'next-turn-ok' }
              else
                edit_response
              end
    finish = message[:tool_calls] ? 'tool_calls' : 'stop'
    if body['stream']
      delta = message.dup
      if delta[:tool_calls]
        delta[:tool_calls] = delta[:tool_calls].each_with_index.map { |call, index| call.merge(index: index) }
      end
      chunks = [
        { id: 'config-fixture', object: 'chat.completion.chunk', model: body['model'], choices: [{ index: 0, delta: delta, finish_reason: nil }] },
        { id: 'config-fixture', object: 'chat.completion.chunk', model: body['model'], choices: [{ index: 0, delta: {}, finish_reason: finish }], usage: { prompt_tokens: 100, completion_tokens: 10, total_tokens: 110 } }
      ].map { |chunk| "data: #{JSON.generate(chunk)}\n\n" }.join + "data: [DONE]\n\n"
      response(socket, 'text/event-stream', chunks)
    else
      response(socket, 'application/json', JSON.generate(id: 'config-fixture', object: 'chat.completion', model: body['model'],
        choices: [{ index: 0, message: message, finish_reason: finish }], usage: { prompt_tokens: 100, completion_tokens: 10, total_tokens: 110 }))
    end
  end

  def edit_response
    @step += 1
    case @step
    when 1
      tool('configuration_read', operation: 'snapshot')
    when 2
      tool('configuration_read', operation: 'plan', params: { edits: [
        { op: 'set', section: 'agent', key: 'compressor_model', value: 'summary-new' },
        { op: 'set', section: 'agent', key: 'compressor_profile', value: 'summary' },
        { op: 'set', section: 'agent', key: 'compressor_context_window', value: 32768 }
      ] })
    when 3
      plan = @outputs.reverse.filter_map { |text| JSON.parse(text) rescue nil }.find { |value| value['plan_id'] }
      return { role: 'assistant', content: 'plan-unavailable' } unless plan
      tool('configuration_write', operation: 'apply', params: { plan_id: plan['plan_id'] })
    else
      text = @mode == :edit ? 'retained-history-fixture ' * 12_000 : 'denied-turn-complete'
      { role: 'assistant', content: text }
    end
  end

  def tool(name, arguments)
    { role: 'assistant', tool_calls: [{ id: "config-call-#{@step}", type: 'function', function: { name: name, arguments: JSON.generate(arguments) } }] }
  end

  def response(socket, type, body)
    socket.write("HTTP/1.1 200 OK\r\nContent-Type: #{type}\r\nContent-Length: #{body.bytesize}\r\nConnection: close\r\n\r\n#{body}")
  end
end

class ConfigChatVerification
  def initialize(binary, output)
    @binary = File.expand_path(binary)
    @output = output
    @checks = []
    @homes = []
  end

  def run
    @provider = ConfigChatProvider.new
    Dir.mktmpdir('wd-config-chat-') do |root|
      @root = root
      env = environment('runtime')
      workspace = File.join(root, 'workspace')
      FileUtils.mkdir_p(workspace)
      path = File.join(root, 'selected config.toml')
      write_config(path, 'full-access')
      decoy = File.join(env.fetch('WILLDEEP_HOME'), 'config.toml')
      File.write(decoy, "version = 1\n# decoy must remain unchanged\n")
      original = File.binread(path)
      first = nil
      check('default Runtime chat exposes built-in configuration tools and saves all three fields') do
        first = JSON.parse(command(env, 'run', '--config', path, '--workspace', workspace, '--output', 'json', 'EDIT_CONFIG'))
        raise 'configuration tools missing' unless %w[configuration_read configuration_write].all? { |name| @provider.tool_names.include?(name) }
        text = File.read(path)
        raise 'compressor model was not saved' unless text.include?('compressor_model = "summary-new"')
        raise 'compressor profile was not saved' unless text.include?('compressor_profile = "summary"')
        raise 'compressor window was not saved' unless text.include?('compressor_context_window = 32768')
        raise 'custom --config wrote the home default' unless File.read(decoy).include?('# decoy must remain unchanged')
        backups = Dir.glob(File.join(env.fetch('WILLDEEP_HOME'), 'plugin-data/willdeep-config/backups/*.bak'))
        raise 'exact backup missing' unless backups.any? { |backup| File.binread(backup) == original }
        raise 'new compressor affected the running Turn' if @provider.models.include?('summary-new')
      end
      check('chat snapshots and approval previews contain no credential or source comments') do
        serialized = JSON.generate(@provider.outputs)
        raise 'sensitive source escaped the host' if serialized.include?('SYNTHETIC_CREDENTIAL_CANARY') || serialized.include?('SOURCE_COMMENT_CANARY')
        raise 'internal rendered candidate escaped the host' if serialized.include?('"candidate"')
      end
      check('ordinary next Runtime Turn uses the newly saved compressor') do
        raise 'first turn did not complete' unless first && first['session_id']
        @provider.reset(:check)
        result = JSON.parse(command(env, 'run', '--config', path, '--workspace', workspace,
          '--session', first.fetch('session_id'), '--output', 'json', 'CHECK_AFTER'))
        raise 'next turn did not complete' unless JSON.generate(result).include?('next-turn-ok')
        raise "new compressor was not used: #{@provider.models.inspect}" unless @provider.models.include?('summary-new')
        raise 'old compressor leaked into new Turn' if @provider.models.include?('summary-old')
      end
      denied_env = environment('denied')
      denied_path = File.join(denied_env.fetch('WILLDEEP_HOME'), 'config.toml')
      write_config(denied_path, 'strict')
      before = File.binread(denied_path)
      @provider.reset(:deny)
      check('strict chat can preview but a refused save leaves WILLDEEP_HOME config unchanged') do
        command(denied_env, 'run', '--local', '--workspace', workspace, '--output', 'json', 'EDIT_CONFIG')
        raise 'refused strict call wrote config' unless File.binread(denied_path) == before
        raise 'refused strict call created a backup' unless Dir.glob(File.join(denied_env.fetch('WILLDEEP_HOME'), 'plugin-data/willdeep-config/backups/*.bak')).empty?
        raise 'snapshot did not use WILLDEEP_HOME' unless @provider.outputs.any? { |text| text.include?(denied_path) }
        preview = @provider.outputs.filter_map { |text| JSON.parse(text) rescue nil }.find { |value| value.is_a?(Hash) && value['plan_id'] }
        raise 'strict chat did not produce a valid preview' unless preview && preview['ok'] == true
        raise 'permission refusal missing' unless @provider.outputs.any? { |text| text.match?(/denied|approval|policy/i) }
      end
    ensure
      @homes.each { |env| command(env, 'daemon', 'stop', allow_failure: true) }
    end
  ensure
    @provider&.close
    report = { suite: 'config-chat-runtime', checks: @checks,
      passed: @checks.count { |item| item[:status] == 'passed' }, failed: @checks.count { |item| item[:status] == 'failed' } }
    FileUtils.mkdir_p(@output)
    File.write(File.join(@output, 'runtime.json'), JSON.pretty_generate(report) + "\n")
    lines = ['# 聊天配置 Runtime 验收', '', "通过 #{report[:passed]}，失败 #{report[:failed]}。", '', '| 用例 | 结果 |', '|---|---|']
    @checks.each { |item| lines << "| #{item[:name]} | #{item[:status]}#{item[:error] ? ': ' + item[:error] : ''} |" }
    File.write(File.join(@output, 'runtime.md'), lines.join("\n") + "\n")
    puts JSON.pretty_generate(report)
  end

  def success? = @checks.all? { |item| item[:status] == 'passed' }

  private

  def environment(name)
    home = File.join(@root, name)
    wd_home = File.join(home, '.willdeep')
    FileUtils.mkdir_p(wd_home)
    env = { 'PATH' => ENV.fetch('PATH'), 'HOME' => home, 'USERPROFILE' => home,
      'WILLDEEP_HOME' => wd_home, 'WILLDEEP_LANGUAGE' => 'en', 'WILLDEEP_LOCAL_DISCOVERY' => '0',
      'XDG_CONFIG_HOME' => File.join(home, '.config'), 'XDG_DATA_HOME' => File.join(home, '.local/share'),
      'XDG_CACHE_HOME' => File.join(home, '.cache'), 'DO_NOT_TRACK' => '1', 'NO_PROXY' => '127.0.0.1,localhost',
      'LANG' => 'en_US.UTF-8', 'TERM' => 'xterm-256color' }
    %w[SYSTEMROOT WINDIR TEMP TMP].each { |key| env[key] = ENV[key] if ENV[key] }
    @homes << env
    env
  end

  def write_config(path, approval)
    File.write(path, <<~TOML)
      version = 1
      default_provider = "fixture"
      # SOURCE_COMMENT_CANARY
      [agent]
      approval = "#{approval}"
      max_turns = 12
      auto_title = false
      input_suggestions = false
      auto_dispatch_read_only = false
      small_model_routing = false
      compressor_model = "summary-old"
      compressor_profile = "summary"
      [providers.fixture]
      provider = "openai-compatible"
      api = "chat-completions"
      api_base = "#{@provider.base_url}"
      api_key = "SYNTHETIC_CREDENTIAL_CANARY"
      model = "main-fixture"
      context_window = 32768
      [providers.summary]
      provider = "openai-compatible"
      api = "chat-completions"
      api_base = "#{@provider.base_url}"
      api_key = "SYNTHETIC_CREDENTIAL_CANARY"
      model = "summary-old"
      context_window = 65536
    TOML
    File.chmod(0o600, path)
  end

  def command(env, *args, allow_failure: false)
    out = err = status = nil
    Open3.popen3(env, @binary, *args, unsetenv_others: true, pgroup: true) do |input, stdout, stderr, process|
      input.close
      readers = [Thread.new { stdout.read }, Thread.new { stderr.read }]
      begin
        status = Timeout.timeout(120) { process.value }
      rescue Timeout::Error
        Process.kill('TERM', -process.pid)
        process.value
        raise 'isolated command timed out'
      ensure
        out, err = readers.map(&:value)
      end
    end
    raise "isolated command failed: #{err.to_s[0, 1500]}" unless allow_failure || status.success?
    out
  end

  def check(name)
    yield
    @checks << { name: name, status: 'passed' }
  rescue StandardError => e
    @checks << { name: name, status: 'failed', error: e.message }
  end
end

verification = ConfigChatVerification.new(ARGV.fetch(0), ARGV.fetch(1))
verification.run
exit(verification.success? ? 0 : 1)
