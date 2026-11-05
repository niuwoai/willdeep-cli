#!/usr/bin/env ruby
# Isolated browser fixture; launch through the desktop preview server manager.
require_relative 'verify_child_sessions'

binary = File.expand_path(ARGV.fetch(0, BIN))
port = Integer(ARGV.fetch(1, '19382'))
output = File.expand_path(ARGV.fetch(2, File.join(ROOT, 'build/verification/child-sessions-p2')))
class ChildSessionPreviewFixture < ChildSessionModelFixture
  def gate(name)
    # The server manager owns this process; cleanup releases the local request.
    @mutex.synchronize { @condition.wait(@mutex, 0.2) until @released[name] }
  end
end

fixture = ChildSessionPreviewFixture.new
stopping = false
%w[TERM INT].each { |signal| Signal.trap(signal) { stopping = true } }

Dir.mktmpdir('wd-child-sessions-', '/private/tmp') do |root|
  home, workspace = File.join(root, 'home'), File.join(root, 'workspace')
  FileUtils.mkdir_p([home, workspace, output], mode: 0o700)
  File.write(File.join(workspace, 'evidence.txt'), "READ_ONLY_EVIDENCE_42\n")
  config = <<~CONFIG
    version=1
    default_provider="fixture"
    [agent]
    max_turns=8
    safety_judge=false
    auto_title=false
    input_suggestions=false
    small_model_routing=false
    auto_dispatch_read_only=false
    [local_model]
    enabled=false
    [providers.fixture]
    provider="openai-compatible"
    api="chat-completions"
    api_base="http://127.0.0.1:#{fixture.port}/v1"
    api_key="local-fixture"
    model="fixture-model"
  CONFIG
  File.write(File.join(home, 'config.toml'), config, perm: 0o600)
  env = workflow_fixture_env(home).merge('WILLDEEP_WORKSPACE_CHECKPOINTS' => '0')
  web = nil
  begin
    child_api(env, 'workspace.register', root: workspace, name: '子会话协作演示', access: 'workspace_write', provider_profile: 'fixture')
    parent = child_api(env, 'session.create', workspace: workspace, title: '整理配置检查结果', profile: 'fixture').fetch('id')
    child_api(env, 'turn.submit', session_id: parent, turn_request_id: SecureRandom.uuid,
      prompt: '整理配置加载顺序，并保留各项调查的独立聊天。', origin_client: 'child-preview')
    wait_for('preview parent idle') { child_api(env, 'session.get', id: parent)['status'] == 'idle' }
    completed = child_api(env, 'session.child.start', parent_session_id: parent,
      title: '核对环境变量优先级', prompt: 'CHILD_P0_READ: read the bounded evidence.', notify: 'none')
    child_terminal(env, parent, completed.fetch('assignment_id'))
    running = child_api(env, 'session.child.start', parent_session_id: parent,
      title: '调查配置文件入口', prompt: 'CHILD_P0_READ HOLD_CHILD', notify: 'none')
    wait_for('preview child executing') do
      fixture.observations.any? { |entry| entry[:child] && entry[:text].include?('HOLD_CHILD') }
    end
    waiting = child_api(env, 'session.child.start', parent_session_id: parent,
      title: '等待配置调查后汇总', prompt: 'CHILD_P0_READ: summarize after the predecessor completes.',
      depends_on: [running.fetch('assignment_id')], subscription: 'inbox')
    File.write(File.join(workspace, 'test'), "puts 'FIXTURE_TEST_PASSED'\n")
    child_git(workspace, 'init', '--quiet')
    child_git(workspace, 'add', 'evidence.txt', 'test')
    child_git(workspace, 'commit', '--quiet', '-m', 'Synthetic browser baseline')
    worktree = child_api(env, 'session.child.start', parent_session_id: parent,
      title: '独立工作树检查', prompt: 'CHILD_P1_WRITE: verify the independent workspace.',
      workspace_mode: 'worktree', subscription: 'none')
    child_terminal(env, parent, worktree.fetch('assignment_id'))
    metadata = { root: root, home: home, workspace: workspace, parent: parent,
      child: completed.fetch('child_session_id'), assignment: completed.fetch('assignment_id'),
      running_child: running.fetch('child_session_id'), running_assignment: running.fetch('assignment_id'),
      waiting_child: waiting.fetch('child_session_id'), waiting_assignment: waiting.fetch('assignment_id'),
      worktree_parent: parent, worktree_child: worktree.fetch('child_session_id'),
      worktree_assignment: worktree.fetch('assignment_id'),
      port: port, url: "http://127.0.0.1:#{port}", loopback_only: true }
    File.write(File.join(output, 'fixture.json'), JSON.pretty_generate(metadata) + "\n")
    web = Process.spawn(env, binary, '--web', '--listen', "127.0.0.1:#{port}",
      '-w', workspace, '-c', env.fetch('WILLDEEP_CONFIG'), '--language', 'zh-CN')
    puts "Child session preview fixture: #{metadata.fetch(:url)}"
    $stdout.flush
    until stopping
      if Process.waitpid(web, Process::WNOHANG)
        code = $?.exitstatus
        web = nil
        raise "isolated Web exited with status #{code}"
      end
      sleep 0.2
    end
  ensure
    fixture.release(:child)
    workflow_finish_child(web)
    begin
      child_stop_daemon(env, binary)
    ensure
      fixture.close
    end
  end
end
