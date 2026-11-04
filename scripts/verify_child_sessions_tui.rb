#!/usr/bin/env ruby
# Attach only to a synthetic child-session fixture, render actual PTY output and preserve evidence.
require_relative 'verify_workflow_tui'


def verify_child_sessions_tui(binary, fixture, output)
  root = File.realpath(fixture.fetch('root'))
  raise 'refusing a non-isolated child fixture' unless root.start_with?('/private/tmp/wd-child-sessions-')
  %w[home workspace].each do |key|
    raise "#{key} escapes the child fixture" unless File.realpath(fixture.fetch(key)) == File.join(root, key)
  end
  env = workflow_fixture_env(fixture.fetch('home')).merge('TERM' => 'xterm-256color',
    'TERM_PROGRAM' => 'child-session-fixture', 'TMUX' => nil)
  parent, child = if fixture['worktree_parent'] && fixture['worktree_child']
    [fixture.fetch('worktree_parent'), fixture.fetch('worktree_child')]
  else
    [fixture.fetch('parent'), fixture.fetch('child')]
  end
  page = workflow_public_api(binary, env, 'session.child.task.list', parent_session_id: parent)
  task = page.fetch('tasks').find { |item| item['child_session_id'] == child }
  raise 'fixture has no child assignment' unless task
  worktree = task.dig('result', 'worktree')
  before = [parent, child].to_h do |id|
    [id, workflow_public_api(binary, env, 'turn.list', session_id: id).map { |turn| turn['id'] }]
  end
  checks = []
  [[110, 34], [42, 22]].each do |columns, rows|
    directory = File.join(output, "tui-#{columns}x#{rows}")
    FileUtils.mkdir_p(directory)
    terminal = nil
    reader, writer, pid = PTY.spawn(env, binary, '--config', env.fetch('WILLDEEP_CONFIG'), '--profile', 'fixture',
      '--workspace', fixture.fetch('workspace'), '--resume', parent, '--language', 'en')
    begin
      terminal = WorkflowPty.new(reader, writer, directory, columns, rows)
      terminal.wait_screen('startup') { |text| text.include?('CHILD_P0_PARENT') || text.include?('CHILD_P1_PARENT') || text.include?('FIXTURE_COMPLETE') || text.include?('PARENT_RESULT_RECEIVED') }
      terminal.input("/session children list\r", 'list child assignments')
      terminal.wait_screen('children-list') do |text|
        compact = text.gsub(/[\s│┃]/, '')
        compact.include?('Assignment:') && compact.include?('/sessionchildrenopen')
      end
      terminal.input("/session children show #{task.fetch('assignment_id')}\r", 'inspect one assignment')
      terminal.wait_screen('children-detail') do |text|
        compact = text.gsub(/[\s│┃]/, '')
        compact.include?(task.fetch('child_session_id')) &&
          (compact.include?('Assignment:') || text.include?('Branch:')) &&
          (worktree.nil? || (text.include?('Branch:') && text.include?('willdeep/child-')))
      end
      terminal.input("/session children open #{child}\r", 'open independent child chat')
      terminal.wait_screen('child-chat') do |text|
        compact = text.gsub(/[\s│┃]/, '')
        text.include?('Task from') && text.include?('Session switched') && compact.include?(child) &&
          (text.include?('CHILD_REPORT') || text.include?('READ_ONLY_EVIDENCE_42'))
      end
      terminal.input("/session children parent\r", 'return to task parent')
      terminal.wait_screen('parent-chat') do |text|
        compact = text.gsub(/[\s│┃]/, '')
        text.include?('Session switched') && compact.include?(parent)
      end
      terminal.input("\x03", 'exit without model input')
      workflow_wait('child fixture TUI exits', 5) do
        terminal.pump(0.05, allow_exit: true)
        Process.waitpid(pid, Process::WNOHANG)
      end
      raise 'TUI exited unsuccessfully' unless $?.success?
      checks << { check: "child_#{worktree ? 'worktree' : 'shared_read_only'}_list_detail_open_parent_#{columns}x#{rows}", passed: true, evidence: directory }
    ensure
      terminal&.save
      reader.close unless reader.closed?
      writer.close unless writer.closed?
      workflow_finish_child(pid)
    end
  end
  after = [parent, child].to_h do |id|
    [id, workflow_public_api(binary, env, 'turn.list', session_id: id).map { |turn| turn['id'] }]
  end
  raise 'navigation commands submitted a model Turn in the parent or child' unless before == after
  report = { passed: true, checks: checks, parent: parent, child: child, terminal_emulator: '@xterm/headless', physical_terminal_gui: false }
  FileUtils.mkdir_p(output)
  File.write(File.join(output, 'tui-report.json'), JSON.pretty_generate(report) + "\n")
  detail = worktree ? "列表、详情显示独立工作树与分支 #{worktree.fetch('branch')}、打开子聊天、返回父聊天" : '列表、详情、打开子聊天、返回父聊天'
  File.write(File.join(output, 'tui-report.md'), "# 子会话 TUI 验收\n\n真实 PTY 与 @xterm/headless 渲染，宽屏与窄屏#{detail}通过；父子会话的导航均未产生模型轮次。未检查实体终端窗口。\n")
  report
rescue StandardError => error
  FileUtils.mkdir_p(output)
  failure = { passed: false, checks: checks || [], error: "#{error.class}: #{error.message}" }
  File.write(File.join(output, 'tui-report.json'), JSON.pretty_generate(failure) + "\n")
  File.write(File.join(output, 'tui-report.md'), "# 子会话 TUI 验收\n\n结果：失败。#{failure[:error]}\n\n已保存实际 PTY 输出；未完成的阶段不计为通过。\n")
  raise
end

if $PROGRAM_NAME == __FILE__
  options = { binary: ENV.fetch('WILLDEEP_BIN', File.expand_path('../target/debug/willdeep', __dir__)) }
  parser = OptionParser.new do |entry|
    entry.on('--fixture PATH') { |path| options[:fixture] = File.expand_path(path) }
    entry.on('--output DIR') { |path| options[:output] = File.expand_path(path) }
    entry.on('--binary PATH') { |path| options[:binary] = File.expand_path(path) }
  end
  parser.parse!
  raise 'supply --fixture PATH from the child Runtime fixture' unless options[:fixture]
  fixture = JSON.parse(File.read(options[:fixture], encoding: Encoding::UTF_8))
  puts JSON.pretty_generate(verify_child_sessions_tui(options[:binary], fixture,
    options.fetch(:output, File.dirname(options[:fixture]))))
end
