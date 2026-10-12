#!/usr/bin/env ruby
# Controlled PTY terminal emulator: records the real TUI protocol output.
require 'pty'
require 'io/console'
require 'json'
require 'base64'
require 'securerandom'
require 'fileutils'

binary, fixture_path, source_session_path, image_path, output = ARGV
fixture = JSON.parse(File.read(fixture_path))
raise 'not an isolated fixture' unless fixture.fetch('root').start_with?('/private/tmp/wdm-')
session = JSON.parse(File.read(source_session_path))
session['id'] = SecureRandom.uuid
session['workspace'] = fixture.fetch('workspace')
session['config'] = fixture.fetch('config')
session['profile'] = 'fixture'
session['title'] = 'Isolated TUI image verification'
session['runtime_managed'] = false
session['execution_checkpoint'] = nil
session['messages'] = [{ role: 'user', content: 'Image fixture', attachments: [{ kind: 'image', name: 'fixture.png', media_type: 'image/png', data: Base64.strict_encode64(File.binread(image_path)), width: 320, height: 180 }] }]
FileUtils.mkdir_p(File.join(fixture.fetch('home'), 'sessions'))
File.write(File.join(fixture.fetch('home'), 'sessions', "#{session.fetch('id')}.json"), JSON.generate(session))
FileUtils.mkdir_p(output)
results = []
%w[halfblocks iterm2].each do |protocol|
  env = { 'WILLDEEP_HOME' => fixture.fetch('home'), 'TERM' => 'xterm-256color', 'TERM_PROGRAM' => protocol == 'iterm2' ? 'iTerm.app' : 'fixture', 'TMUX' => nil, 'WILLDEEP_API_KEY' => nil, 'OPENAI_API_KEY' => nil, 'ANTHROPIC_API_KEY' => nil }
  bytes = ''.b
  stage = 0
  started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  PTY.spawn(env, binary, '--config', fixture.fetch('config'), '--profile', 'fixture', '--workspace', fixture.fetch('workspace'), '--resume', session.fetch('id'), '--language', 'en') do |reader, writer, pid|
    reader.winsize = [30, 110]
    begin
      loop do
        if IO.select([reader], nil, nil, 0.1)
          part = reader.read_nonblock(1_000_000, exception: false)
          break if part.nil?
          unless part == :wait_readable
            bytes << part
            writer.write("\e[6;18;9t\e[0n") if part.include?("\e[16t") || part.include?("\e[5n")
          end
        end
        elapsed = Process.clock_gettime(Process::CLOCK_MONOTONIC) - started
        if stage == 0 && elapsed > 3
          writer.write('/attachments sent' + "\r"); stage = 1
        elsif stage == 1 && elapsed > 5
          writer.write("\r"); stage = 2
        elsif stage == 2 && elapsed > 9
          writer.write("\e"); stage = 3
        elsif stage == 3 && elapsed > 10
          writer.write("\e\x03"); break
        end
        raise 'TUI timed out' if elapsed > 15
      end
    rescue Errno::EIO
      # The child exited normally or was interrupted.
    ensure
      Process.kill('TERM', pid) rescue Errno::ESRCH
      Process.wait(pid) rescue Errno::ECHILD
    end
  end
  File.binwrite(File.join(output, "tui-#{protocol}.ansi"), bytes)
  text = bytes.dup.force_encoding('UTF-8').scrub
  rendered = protocol == 'iterm2' ? bytes.include?("\e]1337;File=") : text.include?('▀') || text.include?('▄')
  raise "#{protocol} image was not rendered; inspect captured ANSI" unless rendered
  result = { check: "real_tui_#{protocol}_image_output", status: 'passed', output_bytes: bytes.bytesize, physical_terminal_gui: false }
  if protocol == 'iterm2'
    encoded = text.match(/\e\]1337;File=[^:]*:([A-Za-z0-9+\/=]+)/)&.captures&.first
    raise 'native image payload missing' unless encoded
    png = Base64.strict_decode64(encoded)
    raise 'native protocol payload is not PNG' unless png.start_with?("\x89PNG".b)
    File.binwrite(File.join(output, 'tui-native-preview.png'), png)
  end
  results << result
end
File.write(File.join(output, 'tui-management.json'), JSON.pretty_generate(version: '0.98.0-rc1', tests: results))
puts JSON.pretty_generate(results)
