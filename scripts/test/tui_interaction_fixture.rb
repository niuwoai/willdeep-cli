# frozen_string_literal: true

require 'base64'
require 'fileutils'
require 'io/console'
require 'json'
require 'open3'
require 'pty'
require 'socket'
require 'timeout'

# Loopback-only synthetic Chat Completions provider. Every increment is released
# by the test so assertions do not depend on model/network timing.
class InteractionProvider
  attr_reader :port

  def initialize
    @server = TCPServer.new('127.0.0.1', 0)
    @port = @server.addr[1]
    @mutex, @requests, @workers, @sockets = Mutex.new, [], [], []
    @listener = Thread.new do
      loop do
        socket = @server.accept
        @sockets << socket
        @workers << Thread.new(socket) { |client| serve(client) }
      end
    rescue IOError, Errno::EBADF
      # The listener is closed explicitly by this fixture.
    end
  end

  def requests
    @mutex.synchronize { @requests.dup }
  end

  def delta(index, text)
    requests.fetch(index).fetch(:commands).push([:delta, text])
  end

  def finish(index)
    requests.fetch(index).fetch(:commands).push([:finish, nil])
  end

  def close
    @server.close unless @server.closed?
    requests.each { |request| request.fetch(:commands).push([:close, nil]) }
    @sockets.each { |socket| socket.close unless socket.closed? }
    @listener.join(1)
    @listener.kill if @listener.alive?
    @workers.each { |worker| worker.join(0.2); worker.kill if worker.alive? }
  end

  private

  def serve(client)
    client.setsockopt(Socket::IPPROTO_TCP, Socket::TCP_NODELAY, 1)
    method, path = client.gets.to_s.split
    headers = {}
    while (line = client.gets) && line != "\r\n"
      key, value = line.split(':', 2)
      headers[key.downcase] = value.strip if value
    end
    body = client.read(headers.fetch('content-length', '0').to_i)
    unless method == 'POST' && path.end_with?('/chat/completions')
      client.write("HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
      return
    end
    payload = JSON.parse(body)
    commands = Queue.new
    @mutex.synchronize { @requests << {messages: payload.fetch('messages'), commands: commands} }
    client.write("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n")
    loop do
      kind, text = commands.pop
      break if kind == :close
      if kind == :delta
        event = {choices: [{index: 0, delta: {content: text}}]}
        client.write("data: #{JSON.generate(event)}\n\n")
      else
        event = {choices: [{index: 0, delta: {}, finish_reason: 'stop'}],
          usage: {prompt_tokens: 1000, completion_tokens: 100, total_tokens: 1100}}
        client.write("data: #{JSON.generate(event)}\n\ndata: [DONE]\n\n")
        break
      end
    end
  rescue IOError, Errno::EPIPE, Errno::ECONNRESET
    # Cancellation/fixture shutdown closes these owned sockets.
  rescue StandardError => error
    warn "interaction provider failed: #{error.class}: #{error.message}"
  ensure
    client.close unless client.closed?
  end
end

class InteractionPty
  FRAME_START = "\e[?2026h"
  FRAME_END = "\e[?2026l"
  OSC52_PREFIX = "\e]52;"

  attr_reader :screen, :raw, :columns, :rows, :frame_serial, :clipboard_writes

  def initialize(binary, environment, config, workspace, session_id, columns: 100, rows: 32, language: 'en')
    raise ArgumentError, 'interaction PTY requires SSH_CONNECTION for isolated OSC 52 copying' if environment.fetch('SSH_CONNECTION', '').empty?
    @columns, @rows = columns, rows
    @reader, @writer, @pid = PTY.spawn(environment, binary, '--config', config,
      '--workspace', workspace, '--language', language, '--resume', session_id,
      unsetenv_others: true)
    @reader.winsize = [rows, columns]
    @decoder_in, @decoder_out, @decoder_err, @decoder_process = Open3.popen3('node',
      File.join(__dir__, 'tui_interaction_frames.mjs'), columns.to_s, rows.to_s)
    @raw, @pending, @queries, @osc52_pending = ''.b, ''.b, ''.b, ''.b
    @clipboard_writes = []
    @frame_serial = 0
    @last_frame_start = nil
    @screen = nil
  end

  def now
    Process.clock_gettime(Process::CLOCK_MONOTONIC)
  end

  def write(text)
    @writer.write(text)
    @writer.flush
  end

  def paste(text)
    write("\e[200~#{text}\e[201~")
  end

  def wheel(up:, count: 1, x: 4, y: 10)
    count.times { write("\e[<#{up ? 64 : 65};#{x};#{y}M") }
  end

  def mouse_down(cell)
    write("\e[<0;#{cell.fetch('x') + 1};#{cell.fetch('y') + 1}M")
  end

  def mouse_move(cell)
    write("\e[<32;#{cell.fetch('x') + 1};#{cell.fetch('y') + 1}M")
  end

  def mouse_up(cell)
    write("\e[<0;#{cell.fetch('x') + 1};#{cell.fetch('y') + 1}m")
  end

  def drag(first, last, release: true)
    mouse_down(first)
    mouse_move(last)
    mouse_up(last) if release
  end

  def click(x, y)
    write("\e[<0;#{x + 1};#{y + 1}M\e[<0;#{x + 1};#{y + 1}m")
  end

  def resize(columns, rows)
    # Drain already available output before marking the resize boundary. An old
    # partial frame must not count as the application's response to WINCH.
    pump(0.1)
    output_boundary, previous_frame = @raw.bytesize, @frame_serial
    @columns, @rows = columns, rows
    @reader.winsize = [rows, columns]
    Process.kill('WINCH', @pid)
    wait('complete application frame after resize', frame_started_after: output_boundary) { true }
    {columns: columns, rows: rows, previous_frame: previous_frame, frame: @frame_serial,
      resize_output_boundary: output_boundary, frame_start: @last_frame_start}
  end

  def pump(seconds = 0.1)
    deadline = now + seconds
    while (remaining = deadline - now).positive?
      next unless IO.select([@reader], nil, nil, [remaining, 0.02].min)
      data = @reader.read_nonblock(1_000_000, exception: false)
      raise 'interaction TUI exited unexpectedly' if data.nil?
      next unless data.is_a?(String)
      @raw << data
      capture_clipboard(data)
      @pending << data
      @queries << data
      @queries.gsub!(/\e\[(16t|5n|6n|c|>c|\?u)/) do
        write({'16t' => "\e[6;18;9t", '5n' => "\e[0n", '6n' => "\e[1;1R",
          'c' => "\e[?1;2c", '>c' => "\e[>0;0;0c", '?u' => "\e[?0u"}.fetch(Regexp.last_match(1)))
        ''
      end
      @queries = @queries.byteslice(-64, 64) || @queries
      while (last = @pending.index(FRAME_END))
        offset = @raw.bytesize - @pending.bytesize
        frame = @pending.slice!(0, last + FRAME_END.bytesize)
        if (start = frame.rindex(FRAME_START))
          @last_frame_start = offset + start
          @frame_serial += 1
        end
        decode(frame)
        @screen['application_frame'] = @frame_serial
        @screen['frame_start_byte'] = @last_frame_start
      end
    end
    @screen
  rescue Errno::EIO
    raise 'interaction TUI exited unexpectedly (PTY EIO)'
  end

  def wait(label, seconds: 25, frame_started_after: nil)
    deadline = now + seconds
    loop do
      fresh_frame = frame_started_after.nil? || (@last_frame_start && @last_frame_start >= frame_started_after)
      return @screen if @screen && fresh_frame && yield(@screen)
      if now > deadline
        visible = @screen&.fetch('lines', [])&.last(8)&.join("\n")
        raise "#{label}: timed out\nLast terminal rows:\n#{visible}"
      end
      pump(0.03)
    end
  end

  def snapshot(directory, name)
    File.write(File.join(directory, "#{name}.json"), JSON.pretty_generate(@screen))
    File.binwrite(File.join(directory, "#{name}.ansi"), @raw)
  end

  def close
    Process.kill('TERM', @pid) rescue Errno::ESRCH
    @reader.close unless @reader.closed?
    @writer.close unless @writer.closed?
    begin
      Timeout.timeout(3) { Process.wait(@pid) }
    rescue Timeout::Error
      Process.kill('KILL', @pid) rescue Errno::ESRCH
      Process.detach(@pid) rescue Errno::ECHILD
    rescue Errno::ECHILD
      # Already reaped by PTY.
    end
    @decoder_in.close unless @decoder_in.closed?
    InteractionProcess.terminate(@decoder_process) unless @decoder_process.join(InteractionProcess::TERM_GRACE)
    @decoder_out.close unless @decoder_out.closed?
    @decoder_err.close unless @decoder_err.closed?
  end

  private

  def capture_clipboard(data)
    # The child's SSH_CONNECTION forces OSC 52. These bytes stay in the private
    # PTY/decoder; they are never forwarded to a real terminal or clipboard API.
    @osc52_pending << data
    @osc52_pending.gsub!(/\e\]52;([^;]*);([^\a\e]*)(?:\a|\e\\)/n) do
      target, payload = Regexp.last_match(1), Regexp.last_match(2)
      unless payload == '?'
        copied = Base64.strict_decode64(payload).force_encoding(Encoding::UTF_8)
        raise 'interaction OSC 52 copy is not valid UTF-8' unless copied.valid_encoding?
        @clipboard_writes << {target: target, text: copied, bytes: copied.bytesize,
          output_end_byte: @raw.bytesize}
      end
      ''
    end
    start = @osc52_pending.index(OSC52_PREFIX)
    @osc52_pending = if start
      @osc52_pending.byteslice(start..)
    else
      @osc52_pending.byteslice(-(OSC52_PREFIX.bytesize - 1), OSC52_PREFIX.bytesize - 1) || @osc52_pending
    end
  end

  def decode(bytes)
    @decoder_in.puts(JSON.generate({bytes: Base64.strict_encode64(bytes), columns: @columns, rows: @rows}))
    @decoder_in.flush
    raise 'interaction VT decoder timed out' unless IO.select([@decoder_out], nil, nil, 3)
    decoded = @decoder_out.gets || raise('interaction VT decoder exited')
    @screen = JSON.parse(decoded.force_encoding(Encoding::UTF_8))
  end
end

module InteractionProcess
  COMMAND_TIMEOUT = 35
  TERM_GRACE = 2
  KILL_GRACE = 3

  def self.terminate(process, group: false)
    target = group ? -process.pid : process.pid
    Process.kill('TERM', target) rescue Errno::ESRCH
    return if process.join(TERM_GRACE)

    Process.kill('KILL', target) rescue Errno::ESRCH
    raise "interaction child #{process.pid} was not reaped after KILL" unless process.join(KILL_GRACE)
  end
end

# Use only the explicitly supplied private fixture HOME; never inherit credentials.
def interaction_command(environment, *command)
  # The block form of popen3 performs an unbounded join in its own ensure.
  input, output, error, process = Open3.popen3(environment, *command, unsetenv_others: true, pgroup: true)
  input.close
  out, err = Thread.new { output.read }, Thread.new { error.read }
  begin
    Timeout.timeout(InteractionProcess::COMMAND_TIMEOUT) do
      status = process.value
      raise "fixture command failed: #{command.last}: #{err.value}" unless status.success?
      out.value
    end
  rescue Timeout::Error
    InteractionProcess.terminate(process, group: true)
    raise "interaction fixture command timed out: #{command.last}"
  ensure
    [out, err].each do |reader|
      reader.join(1)
      reader.kill if reader.alive?
    end
    [input, output, error].each { |stream| stream.close unless stream.closed? }
  end
end
