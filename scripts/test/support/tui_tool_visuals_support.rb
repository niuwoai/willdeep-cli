# frozen_string_literal: true

require 'digest'
require 'rbconfig'
require 'securerandom'
require 'shellwords'
require_relative '../tui_interaction_fixture'

# Independent extension: the existing text-only fixture stays unchanged. Tool
# calls and response termination are released by the acceptance script, never a
# model, timer, or automatic response plan.
class ToolVisualProvider < InteractionProvider
  def initialize
    @errors = Queue.new
    super
  end

  def tool(index, id:, name:, arguments:)
    requests.fetch(index).fetch(:commands).push([:tool, {id: id, name: name, arguments: arguments}])
  end

  def reasoning(index, text)
    requests.fetch(index).fetch(:commands).push([:reasoning, text])
  end

  def assert_healthy!
    raise @errors.pop unless @errors.empty?
  end

  def evidence
    requests.map do |request|
      {messages: request.fetch(:messages), advertised_tools: request.fetch(:advertised_tools)}
    end
  end

  private

  def serve(client)
    client.setsockopt(Socket::IPPROTO_TCP, Socket::TCP_NODELAY, 1)
    method, request_path = client.gets.to_s.split
    headers = {}
    while (line = client.gets) && line != "\r\n"
      key, value = line.split(':', 2)
      headers[key.downcase] = value.strip if value
    end
    body = client.read(Integer(headers.fetch('content-length', '0')))
    unless method == 'POST' && request_path.to_s.end_with?('/chat/completions')
      client.write("HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
      return
    end
    payload = JSON.parse(body)
    commands = Queue.new
    @mutex.synchronize do
      @requests << {messages: payload.fetch('messages'), commands: commands,
        advertised_tools: payload.fetch('tools', []).map { |tool| tool.fetch('function').fetch('name') }}
    end
    client.write("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n")
    loop do
      kind, value = commands.pop
      case kind
      when :close
        break
      when :delta
        emit(client, {content: value})
      when :reasoning
        emit(client, {reasoning_content: value})
      when :tool
        emit(client, {role: 'assistant', tool_calls: [{index: 0, id: value.fetch(:id), type: 'function',
          function: {name: value.fetch(:name), arguments: JSON.generate(value.fetch(:arguments))}}]})
        finish_event(client, 'tool_calls')
        break
      when :finish
        finish_event(client, 'stop')
        break
      else
        raise "unknown controlled Provider command: #{kind.inspect}"
      end
    end
  rescue IOError, Errno::EPIPE, Errno::ECONNRESET
    # Only owned loopback clients; cancellation and close tear these down.
  rescue StandardError => error
    @errors.push("tool visual Provider failed: #{error.class}: #{error.message}")
  ensure
    client.close unless client.closed?
  end

  def emit(client, delta, finish_reason = nil)
    event = {id: 'tool-visual-fixture', object: 'chat.completion.chunk', model: 'fixture',
      choices: [{index: 0, delta: delta, finish_reason: finish_reason}]}
    client.write("data: #{JSON.generate(event)}\n\n")
    client.flush
  end

  def finish_event(client, reason)
    emit(client, {}, reason)
    client.write("data: [DONE]\n\n")
    client.flush
  end
end

# Reuse InteractionPty's VT decoding, complete-frame waits, mouse support, OSC52
# capture and process cleanup. Only spawn setup differs: the decoder also gets
# the private environment, and both children start outside the source checkout.
class ToolVisualPty < InteractionPty
  def initialize(binary, environment, config, workspace, session_id, columns: 132, rows: 70)
    raise 'SSH_CONNECTION is required for private OSC52 capture' if environment.fetch('SSH_CONNECTION', '').empty?
    @columns, @rows = columns, rows
    @reader, @writer, @pid = PTY.spawn(environment, binary, '--config', config,
      '--workspace', workspace, '--language', 'en', '--resume', session_id,
      unsetenv_others: true, chdir: workspace)
    @reader.winsize = [rows, columns]
    @decoder_in, @decoder_out, @decoder_err, @decoder_process = Open3.popen3(environment, 'node',
      File.expand_path('../tui_interaction_frames.mjs', __dir__), columns.to_s, rows.to_s,
      unsetenv_others: true, chdir: environment.fetch('HOME'))
    @raw, @pending, @queries, @osc52_pending = ''.b, ''.b, ''.b, ''.b
    @clipboard_writes, @frame_serial, @last_frame_start, @screen = [], 0, nil, nil
  end
end

# The parent owns an exclusive lock before dispatch. The real shell child
# announces arrival and blocks on a shared lock until a running PTY frame has
# been saved. No time-based sleep, real network or external file is involved.
class ToolVisualCommandGate
  attr_reader :command, :tail, :started, :finished, :output_marker

  def initialize(workspace, index)
    @started = File.join(workspace, "command-#{index}.started")
    @finished = File.join(workspace, "command-#{index}.finished")
    @tail = "WD_COMMAND_TAIL_#{index}_VISIBLE"
    @output_marker = "WD_COMMAND_#{index}_OK"
    name = "command-#{index}.lock"
    @lock = File.open(File.join(workspace, name), File::RDWR | File::CREAT | File::EXCL, 0o600)
    @lock.close_on_exec = true
    @lock.flock(File::LOCK_EX)
    code = "File.open(#{name.inspect}, 'r') { |gate| " \
      "File.write(#{File.basename(@started).inspect}, 'ready'); gate.flock(File::LOCK_SH) }; " \
      "File.write(#{File.basename(@finished).inspect}, 'complete'); puts #{@output_marker.inspect}"
    # The final literal argument is unused by Ruby and appears nowhere in stdout.
    # It is beyond the public 96-character summary even with a short Ruby path.
    @command = [RbConfig.ruby, '-e', code, '--', 'command-visual-padding-' * 8, @tail].shelljoin
    raise 'command tail must be outside the 96-character summary' unless @command.index(@tail) > 96
  end

  def release
    return if @lock.closed?
    @lock.flock(File::LOCK_UN)
    @lock.close
  end
end

module ToolVisualAcceptance
  WAIT_SECONDS = 35
  TIME_LABEL = /\b(?:now|\d+[mhd] ago)\b/

  def self.assert(value, message)
    raise message unless value
    value
  end

  def self.executable(name)
    candidate = ENV.fetch('PATH').split(File::PATH_SEPARATOR)
      .map { |directory| File.join(directory, name) }
      .find { |entry| File.file?(entry) && File.executable?(entry) }
    raise "required executable not found: #{name}" unless candidate
    File.realpath(candidate)
  end

  def self.environment(root)
    user = File.join(root, 'user')
    environment = {
      'PATH' => [File.dirname(executable('node')), File.dirname(RbConfig.ruby),
        '/usr/bin', '/bin', '/usr/sbin', '/sbin'].uniq.join(File::PATH_SEPARATOR),
      'HOME' => user, 'USERPROFILE' => user, 'WILLDEEP_HOME' => File.join(user, '.willdeep'),
      'XDG_CONFIG_HOME' => File.join(user, '.config'), 'XDG_CACHE_HOME' => File.join(user, '.cache'),
      'XDG_DATA_HOME' => File.join(user, '.local/share'), 'TMPDIR' => File.join(root, 'tmp'),
      'TMP' => File.join(root, 'tmp'), 'TEMP' => File.join(root, 'tmp'),
      'GIT_CEILING_DIRECTORIES' => root, 'GIT_CONFIG_NOSYSTEM' => '1', 'GIT_CONFIG_GLOBAL' => File::NULL,
      'TERM' => 'xterm-256color', 'COLORTERM' => 'truecolor', 'LANG' => 'en_US.UTF-8',
      'SSH_CONNECTION' => '127.0.0.1 65535 127.0.0.1 22',
      'WILLDEEP_LOCAL_DISCOVERY' => '0', 'WILLDEEP_NO_UPDATE_CHECK' => '1',
      'WILLDEEP_TELEMETRY_DISABLED' => '1', 'DO_NOT_TRACK' => '1',
      'NO_PROXY' => '127.0.0.1,localhost', 'no_proxy' => '127.0.0.1,localhost'
    }
    %w[HOME WILLDEEP_HOME XDG_CONFIG_HOME XDG_CACHE_HOME XDG_DATA_HOME TMPDIR].each do |key|
      FileUtils.mkdir_p(environment.fetch(key), mode: 0o700)
    end
    environment
  end

  def self.configure(environment, workspace, mode, provider)
    home = environment.fetch('WILLDEEP_HOME')
    FileUtils.mkdir_p(File.join(home, 'sessions'), mode: 0o700)
    session_id = SecureRandom.uuid
    File.write(File.join(home, 'sessions', "#{session_id}.json"), JSON.generate({
      version: 1, id: session_id, title: 'Tool visuals private fixture', workspace: workspace,
      profile: 'fixture', created_at: 1, updated_at: 1, runtime_event_cursor: 0,
      messages: [{role: 'assistant', content: 'TOOL_VISUALS_READY'}]
    }), perm: 0o600)
    config = File.join(home, 'config.toml')
    File.write(config, ['version = 1', "default_provider = 'fixture'", '[agent]',
      "approval = 'full-access'", 'sandbox = false', 'max_turns = 24',
      'auto_title = false', 'input_suggestions = false', 'small_model_routing = false',
      'auto_dispatch_read_only = false', '[ui]', "mode = '#{mode}'", 'check_updates = false',
      'copy_on_select = false', '[providers.fixture]', "provider = 'openai-compatible'",
      "api = 'chat-completions'", "api_base = 'http://127.0.0.1:#{provider.port}/v1'",
      "api_key = 'synthetic-tool-visual-fixture'", "model = 'fixture'", 'context_window = 2000000', ''].join("\n"), perm: 0o600)
    [config, session_id]
  end

  def self.api(environment, binary, config, operation, params)
    argument_file = File.join(environment.fetch('TMPDIR'), "api-#{SecureRandom.hex(6)}.json")
    File.write(argument_file, JSON.generate(params), perm: 0o600)
    result = JSON.parse(interaction_command(environment, binary, '--config', config,
      'api', operation, '--params-file', argument_file))
    assert(result['status'] == 'ok', "#{operation} failed: #{result.inspect}")
    result.fetch('data')
  end

  def self.chat_lines(screen)
    screen.fetch('lines').take_while { |line| !line.start_with?('┌Activity', '┌Prompt') }
  end

  def self.chat(screen)
    chat_lines(screen).join("\n")
  end

  def self.has_tail?(screen, marker)
    # Commands may wrap between any two characters; the vertical frame is not
    # command content. Search only the chat, never the Activity/composer preview.
    chat(screen).gsub(/[\s│]/, '').include?(marker)
  end

  def self.submit(tui, provider, prompt, expected)
    tui.paste(prompt)
    tui.wait('private prompt is painted') { |screen| screen.fetch('lines').join("\n").include?(prompt) }
    tui.write("\r")
    request(tui, provider, expected)
  end

  def self.request(tui, provider, expected)
    tui.wait("Provider request #{expected}", seconds: WAIT_SECONDS) do |_screen|
      provider.assert_healthy!
      count = provider.requests.length
      assert(count <= expected, "unexpected extra Provider request: #{count}, expected #{expected}")
      count == expected
    end
    provider.requests.fetch(expected - 1)
  end

  def self.tool(tui, provider, index, id, name, arguments)
    names = provider.requests.fetch(index).fetch(:advertised_tools)
    assert(names.include?(name), "real Runtime did not advertise #{name}")
    provider.tool(index, id: id, name: name, arguments: arguments)
    following = request(tui, provider, index + 2)
    messages = following.fetch(:messages).select { |message| message['role'] == 'tool' && message['tool_call_id'] == id }
    assert(messages.length == 1, "missing/duplicate real tool receipt for #{name}: #{id}")
    messages.first.fetch('content')
  end

  def self.assert_history(screen)
    rows = chat_lines(screen)
    answer = rows.index { |row| row.include?('HISTORY_AI_LAST_LINE') }
    assert(answer, 'historical AI reply is outside the captured chat')
    divider = ((answer + 1)...rows.length).find { |index| rows[index].include?('turn finished') }
    assert(divider, 'historical turn-finished row is absent')
    assert(divider - answer <= 2, 'historical divider is separated from the AI/time by extra rows')
    assert(rows[(answer + 1)...divider].none? { |row| row.gsub(/[│\s]/, '').empty? }, 'blank row before historical divider')
    assert(rows[answer...divider].join(' ').match?(TIME_LABEL), 'AI time label is absent before historical divider')
    assert(!rows[divider].match?(/your turn/i), 'historical divider still says your turn')
    assert(rows[divider].include?('──'), 'historical ending is not a divider row')
    {answer_row: answer, divider_row: divider, rows: rows[answer..divider]}
  end

  def self.color?(number, kind)
    indexed = kind == :green ? [2, 10] : [1, 9]
    return true if indexed.include?(number)
    return false unless number > 255
    red, green, blue = (number >> 16) & 255, (number >> 8) & 255, number & 255
    kind == :green ? green > red && green > blue : red > green && red > blue
  end

  def self.assert_diff(screen, path, added:, removed:, rows:)
    visible = chat(screen)
    assert(visible.match?(/#{Regexp.escape(path)}\s+\+#{added}\s+[−-]#{removed}(?!\d)/), "missing filename/counts for #{path}")
    colors = []
    rows.each do |spec|
      # Expect literal old/new line numbers and a signed/context body from the
      # actual committed file snapshots, not merely the tool name or arguments.
      left = spec.fetch(:old).to_s
      right = spec.fetch(:new).to_s
      body = spec.fetch(:body)
      pattern = /(?:^|│)\s*#{Regexp.escape(left)}\s*│\s*#{Regexp.escape(right)}\s*│ #{Regexp.escape(body)}/
      y = chat_lines(screen).index { |line| line.match?(pattern) }
      assert(y, "missing numbered diff row for #{path}: #{left}/#{right} #{body}")
      next unless spec[:color]
      marker = body[0]
      cells = screen.fetch('cells').select { |cell| cell.fetch('y') == y && cell.fetch('text') == marker }
      cell = cells.find { |candidate| color?(candidate.fetch('fg'), spec.fetch(:color)) }
      assert(cell, "diff #{marker} has no #{spec.fetch(:color)} cell for #{path}")
      colors << {body: body, x: cell.fetch('x'), y: y, foreground: cell.fetch('fg'), color: spec.fetch(:color)}
    end
    {file: path, added: added, removed: removed, color_cells: colors}
  end
end
