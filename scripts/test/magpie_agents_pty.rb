#!/usr/bin/env ruby
# Run in a separate process so forkpty never inherits the threaded HTTP fixture.
require_relative '../verify_workflow_tui'

options = {}
OptionParser.new do |parser|
  parser.on('--fixture PATH') { |value| options[:fixture] = value }
  parser.on('--binary PATH') { |value| options[:binary] = File.expand_path(value) }
  parser.on('--report-dir PATH') { |value| options[:report] = File.expand_path(value) }
end.parse!
fixture = JSON.parse(File.read(options.fetch(:fixture), encoding: 'UTF-8'))
root = File.realpath(fixture.fetch('root'))
raise 'refusing a non-isolated Magpie PTY fixture' unless root.start_with?('/private/tmp/wd-magpie-agents-')
environment = fixture.fetch('environment')
%w[HOME WILLDEEP_HOME].each do |key|
  raise "#{key} escapes fixture root" unless File.realpath(environment.fetch(key)).start_with?("#{root}/")
end
workspace = File.realpath(fixture.fetch('workspace'))
raise 'workspace escapes fixture root' unless workspace.start_with?("#{root}/")
checks = []
Signal.trap('TERM') { raise 'PTY verification interrupted' }
begin
  [[110, 34], [42, 22]].each do |columns, rows|
    output = File.join(options.fetch(:report), "pty-#{columns}x#{rows}")
    FileUtils.mkdir_p(output)
    reader, writer, pid = PTY.spawn(environment, options.fetch(:binary), '--new-session',
      '--workspace', workspace, '--language', 'en', unsetenv_others: true)
    terminal = WorkflowPty.new(reader, writer, output, columns, rows)
    begin
      terminal.wait_screen('startup') { |text| text.include?('WillDeep') && text.include?('Prompt') && text.include?('Ready') }
      terminal.input("MAGPIE_PTY_TEST\r", 'submit a model turn')
      terminal.wait_screen('reply') { |text| text.include?('magpie-agent-ok') }
      terminal.input("\x03", 'Ctrl-C')
      workflow_wait('TUI exits after completed turn', 5) do
        terminal.pump(0.05, allow_exit: true)
        Process.waitpid(pid, Process::WNOHANG)
      end
      raise 'TUI exited unsuccessfully' unless $?.success?
      checks << { name: "tui_#{columns}x#{rows}", status: 'passed', columns: columns, rows: rows }
    ensure
      begin
        terminal.save
      ensure
        reader.close unless reader.closed?
        writer.close unless writer.closed?
        workflow_finish_child(pid)
      end
    end
  end
rescue StandardError => error
  checks << { name: 'real_tui', status: 'failed', detail: error.message }
ensure
  FileUtils.mkdir_p(options.fetch(:report))
  passed = checks.size == 2 && checks.all? { |check| check[:status] == 'passed' }
  report = { passed: passed, physical_terminal_gui: false, terminal_emulator: '@xterm/headless', checks: checks }
  File.write(File.join(options.fetch(:report), 'pty.json'), JSON.pretty_generate(report) + "\n")
  File.write(File.join(options.fetch(:report), 'pty.md'), "# Magpie Agent 终端验收\n\n结果：#{passed ? '通过' : '失败'}。真实 PTY，110×34 与 42×22，隔离配置、本机模拟模型。\n\n" + checks.map { |entry| "- #{entry[:name]}：#{entry[:status]} #{entry[:detail]}\n" }.join)
  puts JSON.generate(report)
end
exit(passed ? 0 : 1)
