#!/usr/bin/env ruby
# Real PTY checks against the isolated browser fixture; never attach to a personal Daemon.
require_relative 'verify_child_sessions_p2'

def p2_screen_has?(text, expected)
  text.gsub(/[\s│┃]/, '').include?(expected.gsub(/[\s│┃]/, ''))
end

options = { binary: BIN }
OptionParser.new do |parser|
  parser.on('--fixture PATH') { |path| options[:fixture] = File.expand_path(path) }
  parser.on('--output DIR') { |path| options[:output] = File.expand_path(path) }
end.parse!
raise 'supply --fixture PATH' unless options[:fixture]
fixture = JSON.parse(File.read(options[:fixture], encoding: Encoding::UTF_8))
root = File.realpath(fixture.fetch('root'))
raise 'refusing a non-isolated fixture' unless root.start_with?('/private/tmp/wd-child-sessions-')
%w[home workspace].each do |key|
  raise "#{key} escapes fixture" unless File.realpath(fixture.fetch(key)) == File.join(root, key)
end
output = options.fetch(:output, File.join(ROOT, 'build/verification/child-sessions-p2/tui'))
FileUtils.mkdir_p(output)
env = workflow_fixture_env(fixture.fetch('home')).merge('TERM' => 'xterm-256color',
  'TERM_PROGRAM' => 'child-session-p2-fixture', 'TMUX' => nil)
parent = fixture.fetch('parent')
checks = []
begin
  [[110, 34], [42, 22]].each do |columns, rows|
    task = p2_start(env, parent, "PTY control #{columns}", workspace_mode: 'worktree',
      depends_on: [fixture.fetch('running_assignment')])
    raise 'PTY target was not waiting without a Turn' unless task['waiting_on_dependencies'] && task['child_turn_id'].nil?
    child = task.fetch('child_session_id')
    prepared_path = p2_link(env, parent, child).fetch('worktree').fetch('path')
    raise 'waiting worktree was created before user action' if File.exist?(prepared_path)
    before = [parent, child].to_h { |id| [id, child_api(env, 'turn.list', session_id: id).map { |turn| turn['id'] }] }
    directory = File.join(output, "#{columns}x#{rows}")
    FileUtils.mkdir_p(directory)
    terminal = nil
    reader, writer, pid = PTY.spawn(env, BIN, '--config', env.fetch('WILLDEEP_CONFIG'),
      '--profile', 'fixture', '--workspace', fixture.fetch('workspace'), '--resume', parent, '--language', 'en')
    begin
      terminal = WorkflowPty.new(reader, writer, directory, columns, rows)
      terminal.wait_screen('startup') { |text| p2_screen_has?(text,'FIXTURE_COMPLETE') }
      terminal.input("/session children subscription set #{task.fetch('assignment_id')} --subscription inbox --expected-subscription-revision #{task.fetch('subscription_revision')}\r", 'keep result in task list')
      terminal.wait_screen('subscription') { |text| p2_screen_has?(text,'Task list only') }
      raise 'TUI subscription did not persist' unless child_api(env, 'session.child.task.get', parent_session_id: parent, assignment_id: task.fetch('assignment_id'))['subscription'] == 'inbox'
      link = p2_link(env, parent, child)
      command = "/session children takeover #{child} --expected-control-revision #{link.fetch('control_revision')}"
      terminal.input("#{command}\r", 'request takeover explanation without confirmation')
      terminal.wait_screen('confirmation') { |text| p2_screen_has?(text,'Repeat the same command with --yes') }
      raise 'TUI took over without confirmation' unless p2_link(env, parent, child)['control_state'] == 'delegated'
      terminal.input(" --yes\r", 'confirm by appending to preserved command')
      terminal.wait_screen('takeover') { |text| p2_screen_has?(text,'Taken over by user') }
      owned = p2_link(env, parent, child)
      raise 'confirmed takeover missing' unless owned['control_state'] == 'user_owned'
      stale = "#{command} --yes"
      terminal.input("#{stale}\r", 'submit stale control revision')
      terminal.wait_screen('conflict-draft') { |text| p2_screen_has?(text,'Child action failed; input retained') }
      raise 'stale request changed control' unless p2_link(env, parent, child)['control_revision'] == owned['control_revision']
      terminal.input("\x7f" * stale.length, 'remove retained stale draft using Backspace')
      terminal.input("/session children detach #{child} --expected-control-revision #{owned.fetch('control_revision')} --yes\r", 'detach confirmed relationship')
      terminal.wait_screen('detach') { |text| p2_screen_has?(text,'Detached from parent') }
      raise 'TUI detach did not persist' unless p2_link(env, parent, child)['control_state'] == 'detached'
      terminal.input("/session children open #{child}\r", 'open detached chat from historical task')
      terminal.wait_screen('detached-chat') { |text| p2_screen_has?(text,'Session switched') && text.gsub(/[\s│┃]/, '').include?(child) }
      raise 'explicit open did not prepare a real worktree' unless File.directory?(prepared_path)
      raise 'opened directory is not the bound Git worktree' unless File.realpath(child_git(prepared_path, 'rev-parse', '--show-toplevel').strip) == File.realpath(prepared_path)
      raise 'opening restored the blocked assignment' unless child_api(env, 'session.child.task.get', parent_session_id: parent, assignment_id: task.fetch('assignment_id'))['status'] == 'blocked'
      return_command = '/session children parent'
      terminal.input("#{return_command}\r", 'verify detached conversation has no active parent navigation')
      terminal.wait_screen('detached-parent') { |text| p2_screen_has?(text,'This conversation is detached') }
      terminal.input("\x7f" * return_command.length, 'clear preserved navigation command')
      terminal.input("\x03", 'exit without model input')
      workflow_wait('P2 terminal exits', 5) do
        terminal.pump(0.05, allow_exit: true)
        Process.waitpid(pid, Process::WNOHANG)
      end
      raise 'TUI exited unsuccessfully' unless $?.success?
      after = [parent, child].to_h { |id| [id, child_api(env, 'turn.list', session_id: id).map { |turn| turn['id'] }] }
      raise 'control or navigation submitted a model Turn' unless before == after
      checks << { name: "subscription_confirmation_takeover_conflict_detach_navigation_#{columns}x#{rows}", passed: true, evidence: directory }
    ensure
      terminal&.save
      reader.close unless reader.closed?
      writer.close unless writer.closed?
      workflow_finish_child(pid)
    end
  end
  report = { passed: true, checks: checks, terminal_emulator: '@xterm/headless', physical_terminal_gui: false }
rescue StandardError => error
  report = { passed: false, checks: checks, error: "#{error.class}: #{error.message}" }
ensure
  File.write(File.join(output, 'report.json'), JSON.pretty_generate(report) + "\n")
  lines = checks.map { |check| "- #{check[:name]}：通过" }
  File.write(File.join(output, 'report.md'), "# P2 真实 PTY 验收\n\n结果：#{report[:passed] ? '通过' : '失败'}。使用真实 PTY 和 @xterm/headless，无实体终端窗口检查。\n\n#{lines.join("\n")}\n\n#{report[:error]}\n")
end
puts JSON.pretty_generate(report)
exit(report[:passed] ? 0 : 1)
