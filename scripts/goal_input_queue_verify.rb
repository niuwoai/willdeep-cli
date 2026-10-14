#!/usr/bin/env ruby
# Real PTY regression with an isolated local Provider and Runtime; no paid requests.
require 'pty'
require 'io/console'
require 'json'
require 'fileutils'
require 'open3'

binary, metadata_path, output = ARGV
fixture = JSON.parse(File.read(metadata_path))
raise 'not an isolated fixture' unless fixture.fetch('root').start_with?('/private/tmp/wdm-')
FileUtils.mkdir_p(output)
request = '/goal 请读一下 DESIGN.md 并按新设计要求更新一版'
draft = '下一条未发送草稿'
environment = %w[OPENAI_API_KEY ANTHROPIC_API_KEY WILLDEEP_API_KEY WILLDEEP_API_BASE WILLDEEP_MODEL WILLDEEP_CONFIG].to_h { |name| [name, nil] }
environment.merge!('WILLDEEP_HOME' => fixture.fetch('home'), 'TERM' => 'xterm-256color', 'TERM_PROGRAM' => 'fixture', 'TMUX' => nil)
plan = [[3, "hi\r"], [5, "#{request}\r"], [6, draft], [7, "\x11"], [9, "\e"], [16, "\x03"]]
bytes = ''.b
frames = {}
stage = 0
started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
PTY.spawn(environment, binary, '--config', fixture.fetch('config'), '--profile', 'fixture', '--workspace', fixture.fetch('workspace'), '--language', 'zh-CN') do |reader, writer, pid|
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
      frames['queued'] ||= bytes.dup if elapsed >= 8
      frames['after-hi'] ||= bytes.dup if elapsed >= 15
      if stage < plan.length && elapsed >= plan[stage][0]
        writer.write(plan[stage][1])
        stage += 1
      end
      raise 'TUI did not exit within the bounded verification window' if elapsed > 20
    end
  rescue Errno::EIO
    # PTY closes when the application exits.
  ensure
    Process.kill('TERM', pid) rescue Errno::ESRCH
    Process.wait(pid) rescue Errno::ECHILD
  end
end
File.binwrite(File.join(output, 'goal-input-queue.ansi'), bytes)
screens = frames.transform_values.with_index do |frame, index|
  name = index.zero? ? 'queued' : 'after-hi'
  path = File.join(output, "#{name}.ansi")
  File.binwrite(path, frame)
  snapshot, error, status = Open3.capture3('node', File.join(__dir__, 'terminal_snapshot.mjs'), path, File.join(output, "#{name}.txt"))
  raise "terminal snapshot failed: #{error}" unless status.success?
  JSON.parse(snapshot).fetch('text')
end
raise 'queue screen was not captured' unless screens.key?('queued')
raise 'goal request is missing from the queue panel' unless screens['queued'].include?('DESIGN.md')
raise 'queue count is not visible' unless screens['queued'].include?('1/1')
raise 'queue editing action is missing' unless screens['queued'].include?('编辑')
draft_line = screens.fetch('after-hi').lines.find { |line| line.include?(draft) }
raise 'unsent draft changed after hi finished' unless draft_line&.strip&.delete_prefix('│')&.delete_suffix('│')&.strip == draft
sessions = Dir.glob(File.join(fixture.fetch('home'), 'sessions', '*.json')).map { |path| JSON.parse(File.read(path)) }
session = sessions.find { |item| item.fetch('messages').any? { |message| message['role'] == 'user' && message['content'] == 'hi' } }
raise 'hi session missing' unless session
raise 'queued goal was not activated' unless session['goal'] == request.delete_prefix('/goal ')
raise 'queued request was not sent after hi' unless session.fetch('messages').any? { |message| message['role'] == 'user' && message['content'].to_s.include?('DESIGN.md') }
raise 'unsent draft was sent' if session.fetch('messages').any? { |message| message['content'].to_s.include?(draft) }
version, error, status = Open3.capture3(binary, '--version')
raise "version readback failed: #{error}" unless status.success?
report = { version: version.strip, status: 'passed', physical_terminal_gui: false, local_fixture: true,
           checks: %w[hi_running_goal_queued queue_editor_visible goal_activated_after_hi goal_body_submitted unsent_draft_unchanged unsent_draft_not_submitted] }
File.write(File.join(output, 'goal-input-queue.json'), JSON.pretty_generate(report))
File.write(File.join(output, 'goal-input-queue.md'), "# 终端目标输入排队回归\n\n版本：#{version.strip}。真实 PTY 与终端协议解析，使用隔离本地 Provider 和 Runtime；没有调用付费模型，未操作用户运行中的会话。\n\n" + report[:checks].map { |check| "- #{check}：通过" }.join("\n") + "\n\n此验收不证明用户截图中 k8s 文本的来源；它验证回复结束后草稿保持原样。\n")
puts JSON.pretty_generate(report)
