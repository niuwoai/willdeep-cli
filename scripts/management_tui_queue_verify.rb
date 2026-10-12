#!/usr/bin/env ruby
require 'pty'
require 'io/console'
require 'json'
require 'fileutils'
require 'open3'

binary, metadata_path, output = ARGV
fixture = JSON.parse(File.read(metadata_path))
raise 'not an isolated fixture' unless fixture.fetch('root').start_with?('/private/tmp/wdm-')
env = { 'WILLDEEP_HOME' => fixture.fetch('home'), 'TERM' => 'xterm-256color', 'TERM_PROGRAM' => 'fixture', 'TMUX' => nil, 'OPENAI_API_KEY' => nil, 'ANTHROPIC_API_KEY' => nil, 'WILLDEEP_API_KEY' => nil }
plan = [[3, "slow tui queue verification\r"], [5, "\e[200~first attachment\nsecond line\e[201~first queued\r"], [6, "\e[200~second attachment\nsecond line\e[201~second queued\r"], [7, "unsent draft\x11"], [8, "\r\e[Hedited-\r"], [9, 'a'], [10, "\r"], [11, "\e"], [12, "e\e[Hedited attachment: "], [13, "\r"], [14, "\e"], [15, "\e[Bd"], [25, "\e"], [27, "\eOR"], [28, "\e"], [29, "\x03"]]
bytes = ''.b
stage = 0
started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
PTY.spawn(env, binary, '--config', fixture.fetch('config'), '--profile', 'fixture', '--workspace', fixture.fetch('workspace'), '--language', 'en') do |reader, writer, pid|
  reader.winsize = [30, 100]
  begin
    loop do
      if IO.select([reader], nil, nil, 0.05)
        part = reader.read_nonblock(1_000_000, exception: false)
        break if part.nil?
        unless part == :wait_readable
          bytes << part
          writer.write("\e[6;18;9t\e[0n") if part.include?("\e[16t") || part.include?("\e[5n")
        end
      end
      elapsed = Process.clock_gettime(Process::CLOCK_MONOTONIC) - started
      if stage < plan.length && elapsed >= plan[stage][0]
        writer.write(plan[stage][1]); stage += 1
      end
      break if elapsed > 31
    end
  rescue Errno::EIO
  ensure
    Process.kill('TERM', pid) rescue Errno::ESRCH
    Process.wait(pid) rescue Errno::ECHILD
  end
end
FileUtils.mkdir_p(output)
File.binwrite(File.join(output, 'tui-queue.ansi'), bytes)
text = bytes.dup.force_encoding('UTF-8').scrub.gsub(/\e\[[0-?]*[ -\/]*[@-~]/, '')
checks = ['Queued 2', 'Auto-send paused while managing', 'Queued message attachments']
normalized = text.gsub(/\s+/, '')
checks.each { |value| raise "missing TUI evidence: #{value}" unless normalized.include?(value.gsub(/\s+/, '')) }
snapshot, error, status = Open3.capture3('node', File.join(__dir__, 'terminal_snapshot.mjs'), File.join(output, 'tui-queue.ansi'), File.join(output, 'tui-final-screen.txt'))
raise "terminal snapshot failed: #{error}" unless status.success?
raise 'original draft lost' unless JSON.parse(snapshot).fetch('text').include?('unsent draft')
sessions = Dir.glob(File.join(fixture.fetch('home'), 'sessions', '*.json')).map { |path| JSON.parse(File.read(path)) }
session = sessions.find { |item| item.fetch('messages').any? { |message| message['content'] == 'edited-first queued' } }
raise 'edited queued message was not sent' unless session
message = session.fetch('messages').find { |item| item['content'] == 'edited-first queued' }
raise 'edited attachment was not sent with its message' unless message.fetch('attachments').first.fetch('content').include?('edited attachment: ')
raise 'deleted queued message was sent' if session.fetch('messages').any? { |item| item['content'] == 'second queued' }
report = { version: '0.98.0-rc1', status: 'passed', checks: checks, edited_queued_message_sent: true, edited_attachment_sent: true, deleted_message_not_sent: true, original_draft_kept: true }
File.write(File.join(output, 'tui-queue.json'), JSON.pretty_generate(report))
puts JSON.pretty_generate(report)
