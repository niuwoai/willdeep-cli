# frozen_string_literal: true

require 'digest'
require 'fileutils'
require 'json'
require 'net/http'
require 'open3'
require 'securerandom'
require 'socket'
require 'time'
require 'timeout'
require 'uri'
require_relative '../tui_interaction_fixture'

# Private helpers only. Runtime execution always enters through the real binary
# or Web HTTP API; this module never calls a ToolRegistry or the CDP bridge.
module BrowserRuntime
  DEADLINE = 45
  POLL_INTERVAL = 0.04
  ROOT = File.expand_path('../../..', __dir__)
  TERMINAL = %w[completed partial failed cancelled interrupted].freeze

  def self.assert(value, message)
    raise message unless value
    value
  end

  def self.now = Process.clock_gettime(Process::CLOCK_MONOTONIC)

  # The interval only limits observable-state polling. No action is released by
  # elapsed time: every gate below requires a Provider, HTTP, Runtime or PTY fact.
  def self.await(label, seconds: DEADLINE, pump: nil)
    deadline = now + seconds
    loop do
      result = yield
      return result if result
      raise Timeout::Error, "#{label}: timed out after #{seconds}s" if now >= deadline
      if pump
        pump.call
      else
        IO.select(nil, nil, nil, [POLL_INTERVAL, deadline - now].min)
      end
    end
  end

  def self.environment(root, node)
    home = File.join(root, 'home')
    env = {
      'PATH' => "#{File.dirname(node)}:/usr/bin:/bin:/usr/sbin:/sbin",
      'HOME' => home, 'USERPROFILE' => home, 'WILLDEEP_HOME' => File.join(home, '.willdeep'),
      'XDG_CONFIG_HOME' => File.join(home, '.config'), 'XDG_DATA_HOME' => File.join(home, '.local/share'),
      'XDG_CACHE_HOME' => File.join(home, '.cache'), 'TMPDIR' => File.join(root, 'tmp'),
      'TMP' => File.join(root, 'tmp'), 'TEMP' => File.join(root, 'tmp'),
      'TERM' => 'xterm-256color', 'LANG' => 'en_US.UTF-8',
      'WILLDEEP_LOCAL_DISCOVERY' => '0', 'WILLDEEP_TELEMETRY_DISABLED' => '1', 'DO_NOT_TRACK' => '1',
      'NO_PROXY' => '127.0.0.1,localhost', 'no_proxy' => '127.0.0.1,localhost',
      'SSH_CONNECTION' => '127.0.0.1 40000 127.0.0.1 22'
    }
    %w[HOME WILLDEEP_HOME XDG_CONFIG_HOME XDG_DATA_HOME XDG_CACHE_HOME TMPDIR].each do |key|
      FileUtils.mkdir_p(env.fetch(key), mode: 0o700)
    end
    env
  end

  def self.http(base, path, body: nil, method: nil, expected: 200)
    uri = URI.join(base + '/', path)
    assert(uri.host == '127.0.0.1' && uri.scheme == 'http' && uri.port != 9847, 'private loopback HTTP required')
    klass = method || (body.nil? ? Net::HTTP::Get : Net::HTTP::Post)
    request = klass.new(uri)
    request['Origin'] = base
    if body
      request['Content-Type'] = 'application/json'
      request.body = JSON.generate(body)
    end
    response = Net::HTTP.start(uri.host, uri.port, nil, open_timeout: 3, read_timeout: 10) { |client| client.request(request) }
    assert(response.code.to_i == expected, "#{request.method} #{path}: HTTP #{response.code}: #{response.body.to_s.byteslice(0, 800)}")
    response.body.to_s.empty? ? nil : JSON.parse(response.body)
  end

  # Only owned descendants are returned. Never put the process table's command
  # strings in an artifact: unrelated processes may contain user arguments.
  def self.processes
    out, err, status = Open3.capture3({'LC_ALL' => 'C'}, '/bin/ps', '-axo', 'pid=,ppid=,pgid=,state=', unsetenv_others: true)
    assert(status.success?, "ps failed: #{err}")
    out.lines.map do |line|
      pid, parent, group, state = line.split
      {pid: pid.to_i, ppid: parent.to_i, pgid: group.to_i, state: state}
    end
  end

  def self.descendants(root_pid, table = processes)
    selected = []
    parents = [root_pid]
    loop do
      children = table.select { |row| parents.include?(row[:ppid]) && !selected.any? { |old| old[:pid] == row[:pid] } }
      break if children.empty?
      selected.concat(children)
      parents = children.map { |row| row[:pid] }
    end
    selected
  end

  def self.running(rows)
    rows.reject { |row| row[:state].to_s.start_with?('Z') }
  end

  class Command
    attr_reader :pid
    def initialize(env, *argv, input: nil, directory: nil)
      @input, @out, @err, @process = Open3.popen3(env, *argv, unsetenv_others: true, pgroup: true, chdir: directory || env.fetch('HOME'))
      @pid = @process.pid
      @readers = [Thread.new { @out.read }, Thread.new { @err.read }]
      @input.write(input) if input
      @input.close
    end

    def finished? = !@process.alive?
    def signal(name) = Process.kill(name, @pid)

    def result(seconds: DEADLINE)
      raise Timeout::Error, 'private binary command did not finish' unless @process.join(seconds)
      raise Timeout::Error, 'private binary output pipe did not close' unless @readers.all? { |reader| reader.join(3) }
      status = @process.value
      {success: status.success?, exit_code: status.exitstatus, signal: status.termsig,
       stdout: @readers[0].value, stderr: @readers[1].value}
    end

    def close
      InteractionProcess.terminate(@process, group: true) if @process.alive?
    ensure
      [@input, @out, @err].each { |io| io.close unless io.closed? }
      @readers.each { |thread| thread.join(1); thread.kill if thread.alive? }
    end
  end

  def self.command(env, binary, *argv, input: nil)
    process = Command.new(env, binary, *argv, input: input)
    result = process.result
    assert(result[:success], "binary #{argv.first}: exit #{result[:exit_code]} #{result[:stderr]}")
    result[:stdout]
  ensure
    process&.close
  end

  def self.api(manifest, operation, params = {})
    text = command(manifest.fetch('environment'), manifest.fetch('binary'), 'api', operation,
                   '--params-file', '-', input: JSON.generate(params))
    result = JSON.parse(text)
    assert(result['status'] == 'ok', "#{operation}: #{result}")
    result.fetch('data')
  end

  class Pty < InteractionPty
    attr_reader :pid
    # Reuse the existing PTY terminal-query/OSC52/frame protocol. Only dependency
    # selection differs: use the caller's fixed Node, never PATH discovery.
    def initialize(manifest, session)
      @columns, @rows = 132, 42
      env = manifest.fetch('environment')
      BrowserRuntime.assert(!env.fetch('SSH_CONNECTION').empty?, 'private OSC52 transport required')
      @reader, @writer, @pid = PTY.spawn(env, manifest.fetch('binary'), '--config', manifest.fetch('config'),
        '--workspace', manifest.fetch('workspace'), '--language', 'en', '--resume', session,
        unsetenv_others: true, chdir: manifest.fetch('workspace'))
      @reader.winsize = [@rows, @columns]
      @decoder_in, @decoder_out, @decoder_err, @decoder_process = Open3.popen3(env, manifest.fetch('node'),
        File.join(ROOT, 'scripts/test/tui_interaction_frames.mjs'), @columns.to_s, @rows.to_s,
        unsetenv_others: true, chdir: env.fetch('HOME'))
      @raw, @pending, @queries, @osc52_pending = ''.b, ''.b, ''.b, ''.b
      @clipboard_writes, @frame_serial, @last_frame_start, @screen = [], 0, nil, nil
    end

    def text = @screen&.fetch('lines', [])&.join("\n").to_s

    def submit(prompt)
      wait('TUI initial frame') { |screen| screen.fetch('lines').join("\n").include?('Focus') }
      paste(prompt)
      wait('TUI prompt before Enter') { |screen| screen.fetch('lines').last(7).join("\n").include?(prompt) }
      write("\r")
    end

    def cancel_execution
      write("\e")
      wait('TUI first Esc acknowledgement', seconds: 3) { |screen| screen.fetch('lines').join("\n").include?('Press Esc again') }
      write("\e")
    end
  end

  class Stream
    attr_reader :events
    def initialize(base, path, body)
      @events, @mutex, @error, @connection = [], Mutex.new, nil, nil
      @thread = Thread.new do
        uri = URI.join(base + '/', path)
        request = Net::HTTP::Post.new(uri)
        request['Content-Type'], request['Origin'] = 'application/json', base
        request.body = JSON.generate(body)
        client = Net::HTTP.new(uri.host, uri.port, nil)
        client.open_timeout, client.read_timeout = 3, DEADLINE * 3
        @connection = client
        client.start do
          client.request(request) do |response|
            BrowserRuntime.assert(response.code == '200', "Web stream HTTP #{response.code}")
            @decoder = SseDecoder.new { |event| @mutex.synchronize { @events << event } }
            response.read_body { |chunk| @decoder.feed(chunk) }
          end
        end
      rescue StandardError => error
        @error = error
      end
    end

    def snapshot
      raise @error if @error
      @mutex.synchronize { @events.dup }
    end

    def finish
      raise Timeout::Error, 'Web SSE did not close after terminal event' unless @thread.join(10)
      raise @error if @error
      snapshot
    end

    def close
      @connection.finish if @connection&.started?
    rescue IOError, Errno::EBADF
      # The streaming reader already closed its owned socket.
    ensure
      @thread.join(1)
      @thread.kill if @thread.alive?
    end
  end

  class SseDecoder
    def initialize(&emit)
      @buffer, @emit = ''.b, emit
    end

    def feed(bytes)
      @buffer << bytes.b
      while (boundary = @buffer.index(/\r?\n\r?\n/))
        delimiter = @buffer.byteslice(boundary, 4).start_with?("\r\n\r\n") ? 4 : 2
        frame = @buffer.slice!(0, boundary + delimiter)
        data = frame.lines.filter_map { |line| line.sub(/\Adata: ?/, '').strip if line.start_with?('data:') }.join("\n")
        next if data.empty?
        @emit.call(JSON.parse(data.force_encoding(Encoding::UTF_8)))
      end
    end
  end
end
