#!/usr/bin/env ruby
# Real keyboard/mouse verification in an isolated TUI; no prompt is submitted.
require 'pty'
require 'io/console'
require 'tmpdir'
require 'fileutils'
require 'json'
require 'timeout'
require 'open3'

binary = File.expand_path(ARGV.fetch(0))
root = Dir.mktmpdir('wd-overlay-close-', '/private/tmp')
home = File.join(root, 'home')
workspace = File.join(root, 'workspace')
FileUtils.mkdir_p([home, workspace])
config = File.join(home, 'config.toml')
File.write(config, "version = 1\ndefault_provider = 'fixture'\n[agent]\nauto_title = false\ninput_suggestions = false\n[providers.fixture]\nprovider = 'openai-compatible'\napi_base = 'http://127.0.0.1:1/v1'\napi_key = 'fixture-only'\nmodel = 'fixture'\n")
File.chmod(0600, config)
env = {'WILLDEEP_HOME' => home, 'TERM' => 'xterm-256color', 'WILLDEEP_TELEMETRY_DISABLED' => '1'}
%w[WILLDEEP_API_KEY WILLDEEP_API_BASE WILLDEEP_MODEL OPENAI_API_KEY OPENAI_BASE_URL ANTHROPIC_API_KEY ANTHROPIC_BASE_URL].each { |key| env[key] = nil }
all = ''.b
visible = ''.b
stage = 0
draft = 'DRAFT_KEEP_AFTER_MOUSE_CLOSE'
begin
  PTY.spawn(env, binary, '--config', config, '--workspace', workspace, '--language', 'en', '--new-session') do |reader, writer, pid|
    reader.winsize = [30, 100]
    started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    begin
      loop do
        if IO.select([reader], nil, nil, 0.05)
          part = reader.read_nonblock(1_000_000, exception: false)
          raise 'TUI closed unexpectedly' if part.nil?
          if part.is_a?(String)
            all << part
            visible << part
            writer.write("\e[6;18;9t\e[0n") if part.include?("\e[16t") || part.include?("\e[5n")
          end
        end
        if stage.zero? && visible.include?('Hello,')
          writer.write("#{draft}\eOP") # F1 opens help without submitting the draft.
          visible.clear
          stage = 1
        elsif stage == 1 && visible.include?('Keyboard')
          # The rendered title starts at x=7; its Esc label starts at x=28 (0-based).
          writer.write("\e[<0;30;2M\e[<0;30;2m")
          visible.clear
          stage = 2
        elsif stage == 2 && visible.include?('_AFTER_MOUSE_CLOSE')
          break
        end
        raise "verification timed out at stage #{stage}" if Process.clock_gettime(Process::CLOCK_MONOTONIC) - started > 15
      end
    ensure
      File.binwrite(File.join(root, 'tui.ansi'), all)
      Process.kill('TERM', pid) rescue Errno::ESRCH
      begin
        Timeout.timeout(2) { Process.wait(pid) }
      rescue Timeout::Error
        Process.kill('KILL', pid) rescue Errno::ESRCH
      rescue Errno::ECHILD
      end
    end
  end
  snapshot, error, status = Open3.capture3('node', File.join(__dir__, 'terminal_snapshot.mjs'), File.join(root, 'tui.ansi'), File.join(root, 'screen.txt'))
  raise "terminal snapshot failed: #{error}" unless status.success?
  screen = JSON.parse(snapshot).fetch('text')
  raise 'draft was not preserved on screen' unless screen.include?(draft)
  raise 'help did not close' if screen.include?('Keyboard help')
  raise 'click submitted the draft' unless Dir[File.join(home, 'sessions', '*.json')].empty?
  report = {root: root, mouse_closed_help: stage == 2, draft_preserved: screen.include?(draft), no_session_created: true, passed: true}
  File.write(File.join(root, 'report.json'), JSON.pretty_generate(report))
  File.write(File.join(root, 'report.md'), "# Overlay close verification\n\n" + report.map { |key, value| "- #{key}: #{value}" }.join("\n") + "\n")
  puts report.to_json
ensure
  system(env, binary, 'daemon', 'stop', out: File::NULL, err: File::NULL)
end
