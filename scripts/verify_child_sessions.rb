#!/usr/bin/env ruby
# Real Runtime and deterministic local model; all state belongs to a fresh fixture.
require_relative 'verify_session_collaboration'
require_relative 'verify_workflow_tui'
require 'optparse'

class ChildSessionModelFixture < ModelFixture
  def initialize
    @mutex, @condition = Mutex.new, ConditionVariable.new
    @observed, @released = [], {}
    super
  end

  def observations = @mutex.synchronize { @observed.map(&:dup) }

  def release(gate)
    @mutex.synchronize { @released[gate] = true; @condition.broadcast }
  end

  def gate(name)
    @mutex.synchronize do
      deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + 50
      until @released[name]
        remaining = deadline - Process.clock_gettime(Process::CLOCK_MONOTONIC)
        raise "local fixture gate timed out: #{name}" if remaining <= 0
        @condition.wait(@mutex, [remaining, 0.2].min)
      end
    end
  end

  def completion(request)
    messages = request.fetch('messages', [])
    text = JSON.generate(messages)
    names = request.fetch('tools', []).map { |tool| tool.dig('function', 'name') }
    calls = messages.flat_map { |message| message.fetch('tool_calls', []) }
    called = calls.map { |call| call.dig('function', 'name') }
    child = (text.include?('CHILD_P0_READ') || text.include?('CHILD_P1_WRITE')) && !names.include?('start_session')
    writable = child && text.include?('CHILD_P1_WRITE')
    @mutex.synchronize { @observed << { child: child, writable: writable, text: text, tools: names, model: request['model'] } }
    call, content = nil, 'FIXTURE_COMPLETE: local evidence verified.'
    if child
      raise 'fresh child inherited private parent history' if text.include?('PARENT_PRIVATE_SENTINEL')
      forbidden = if writable
        %w[start_session continue_session retry_session_task workflow_control production.start screenplay.start schedule.create]
      else
        %w[start_session continue_session retry_session_task spawn_agent continue_agent retry_agent workflow_control]
      end
      raise "child exposes delegation tools: #{names & forbidden}" unless (names & forbidden).empty?
      gate(:child) if text.include?('HOLD_CHILD')
      gate(:stopped_child) if text.include?('STOP_CHILD')
      gate(:capacity) if text.include?('CAP_CHILD')
      if writable
        if !called.include?('read_file')
          call = ['read_file', { path: 'evidence.txt' }]
        elsif !called.include?('create_file')
          call = ['create_file', { path: 'child-output.txt', content: 'CHILD_WROTE' }]
        elsif !called.include?('run_command')
          call = ['run_command', { command: 'ruby test' }]
        elsif text.include?('CHILD_P1_WORKER') && !called.include?('spawn_agent')
          call = ['spawn_agent', { profile: 'generalist', prompt: 'WORKER_P1 report the bounded result', run_in_background: true }]
        elsif text.include?('CHILD_P1_WORKER') && !called.include?('await_agents')
          call = ['await_agents', { timeout_seconds: 60 }]
        else
          raise 'writable child did not receive real file evidence' unless text.include?('READ_ONLY_EVIDENCE_42')
          content = 'CHILD_REPORT: READ_ONLY_EVIDENCE_42 was read and child-output.txt was written.'
        end
      else
        effects = [
          ['run_command', { command: 'touch child-forbidden' }],
          ['create_file', { path: 'child-forbidden', content: 'must be rejected' }],
          ['spawn_agent', { profile: 'generalist', prompt: 'must be rejected' }],
          ['call_mcp_tool', { server: 'fixture', tool: 'write', arguments: {} }],
          ['start_session', { title: 'Recursive task', prompt: 'must be rejected' }]
        ]
        denied_probe = effects.find { |name, _| text.include?("ATTEMPT_EFFECTS_#{name}") && !called.include?(name) }
        if denied_probe
          call = denied_probe
        elsif !called.include?('read_file')
          call = ['read_file', { path: 'evidence.txt' }]
        else
          raise 'child did not receive real file evidence' unless text.include?('READ_ONLY_EVIDENCE_42')
          content = 'CHILD_REPORT: READ_ONLY_EVIDENCE_42 was read from the shared workspace.'
        end
      end
    elsif text.include?('WORKER_P1')
      content = 'WORKER_REPORT: bounded worker completed.'
    elsif text.include?('CHILD_P0_PARENT') && !called.include?('start_session')
      prompt = text.include?('STOP_SCENARIO') ? 'CHILD_P0_READ STOP_CHILD' : 'CHILD_P0_READ HOLD_CHILD'
      call = ['start_session', { title: '核对只读证据', prompt: prompt, context: 'fresh' }]
    elsif text.include?('CHILD_P1_PARENT') && !called.include?('start_session')
      base = text.include?('CHILD_P1_SNAPSHOT') ? 'snapshot' : 'head'
      prompt = 'CHILD_P1_WRITE CHILD_P1_WORKER: read evidence, write child-output.txt and delegate one bounded worker.'
      call = ['start_session', {
        title: "独立工作树验证 #{base}", prompt: prompt, context: 'fresh',
        workspace_mode: 'worktree', worktree_base: base
      }]
    elsif text.include?('STOP_SCENARIO') && !text.include?('<host-context')
      gate(:parent)
      content = 'PARENT_STOP_SHOULD_NOT_COMMIT'
    elsif text.include?('<host-context')
      content = 'PARENT_RESULT_RECEIVED: the child report is available for review.'
    end
    message = { role: 'assistant', content: call ? nil : content }
    if call
      message[:tool_calls] = [{ id: "call_#{SecureRandom.hex(8)}", type: 'function',
        function: { name: call[0], arguments: JSON.generate(call[1]) } }]
    end
    { id: "chatcmpl-#{SecureRandom.hex(8)}", object: 'chat.completion', model: request['model'] || 'fixture-model',
      choices: [{ index: 0, message: message, finish_reason: call ? 'tool_calls' : 'stop' }],
      usage: { prompt_tokens: 100, completion_tokens: 20, total_tokens: 120 } }
  end

  # The P1 model-override preflight reads the provider's model list and the
  # gateway capability table. Keep both deterministic and local.
  def fixture_list(first)
    if first.include?('model-capabilities')
      {
        models: [
          { id: 'fixture-model', context_window: 128_000, input_modalities: ['text'] },
          { id: 'child-override', context_window: 64_000, input_modalities: ['text'], supports_tools: true }
        ]
      }
    else
      {
        object: 'list',
        data: [
          { id: 'fixture-model', object: 'model', owned_by: 'fixture' },
          { id: 'child-override', object: 'model', owned_by: 'fixture' }
        ]
      }
    end
  end
end

def child_api_response(env, operation, params = {}, request_id: nil)
  arguments = [BIN, 'api', operation, '--params-file', '-', '--ndjson']
  arguments.concat(['--request-id', request_id]) if request_id
  output, error, status = Open3.capture3(env, *arguments, stdin_data: JSON.generate(params))
  output = output.dup.force_encoding(Encoding::UTF_8)
  response = JSON.parse(output)
  raise "#{operation}: #{error} #{output}" unless response.is_a?(Hash) && response.key?('status')
  [response, status.success?]
rescue JSON::ParserError => error
  raise "#{operation}: invalid CLI response: #{error.message}; #{output}"
end

def child_api(env, operation, params = {}, request_id: nil, **keywords)
  response, = child_api_response(env, operation, params.merge(keywords), request_id: request_id)
  raise "#{operation}: #{response}" unless response['status'] == 'ok'
  response.fetch('data')
end

def child_refused(env, operation, params = {}, request_id: nil, **keywords)
  response, = child_api_response(env, operation, params.merge(keywords), request_id: request_id)
  raise "#{operation} unexpectedly allowed: #{response}" if response['status'] == 'ok'
  response
end

def child_cli(env, *arguments)
  output, error, status = Open3.capture3(env, BIN, 'session', 'child', *arguments)
  raise "child CLI #{arguments.first}: #{error} #{output}" unless status.success?
  JSON.parse(output.dup.force_encoding(Encoding::UTF_8))
end

def child_tasks(env, parent)
  child_api(env, 'session.child.task.list', parent_session_id: parent).fetch('tasks')
end

def child_terminal(env, parent, assignment)
  wait_for('child assignment terminal') do
    task = child_api(env, 'session.child.task.get', parent_session_id: parent, assignment_id: assignment)
    task if %w[completed partial failed cancelled interrupted blocked].include?(task['status'])
  end
end

def child_settle(env, parent, assignment)
  wait_for('durable parent result consumption') do
    child_api(env, 'session.child.task.get', parent_session_id: parent, assignment_id: assignment).dig('result', 'consumed_at')
  end
  wait_for('parent idle after child result') { child_api(env, 'session.get', id: parent)['status'] == 'idle' }
end

def child_git(directory, *arguments)
  output, error, status = Open3.capture3(
    {
      'GIT_AUTHOR_NAME' => 'WillDeep Fixture',
      'GIT_AUTHOR_EMAIL' => 'fixture@example.invalid',
      'GIT_COMMITTER_NAME' => 'WillDeep Fixture',
      'GIT_COMMITTER_EMAIL' => 'fixture@example.invalid'
    },
    'git', '-C', directory, *arguments
  )
  raise "git #{arguments.join(' ')}: #{error} #{output}" unless status.success?
  output
end

def child_report(output, report)
  FileUtils.mkdir_p(output)
  File.write(File.join(output, 'report.json'), JSON.pretty_generate(report) + "\n")
  lines = report.fetch(:checks).map { |check| "- #{check[:name]}：#{check[:passed] ? '通过' : '失败'}" }
  File.write(File.join(output, 'report.md'), "# 完整子会话 P0/P1 隔离验收\n\n版本：#{report[:version]}\n\n结果：#{report[:passed] ? '通过' : '失败'}。真实 Runtime、临时 WILLDEEP_HOME、回环假模型，无远端付费调用。\n\n#{lines.join("\n")}\n\n#{report[:error]}\n")
end

if $PROGRAM_NAME == __FILE__
  options = { output: File.join(ROOT, 'build/verification/child-sessions'), port: 19381 }
  OptionParser.new do |parser|
    parser.on('--serve', 'Keep the isolated Web fixture alive for browser verification') { options[:serve] = true }
    parser.on('--port PORT', Integer) { |port| options[:port] = port }
    parser.on('--output DIR') { |directory| options[:output] = File.expand_path(directory) }
  end.parse!
  fixture, checks = ChildSessionModelFixture.new, []
  report = { passed: false, checks: checks, loopback_only: true }
  output = options[:output]
  Dir.mktmpdir('wd-child-sessions-', '/private/tmp') do |root|
    home, workspace = File.join(root, 'home'), File.join(root, 'workspace')
    FileUtils.mkdir_p([home, workspace], mode: 0o700)
    File.write(File.join(workspace, 'evidence.txt'), "READ_ONLY_EVIDENCE_42\n")
    config = "version=1\ndefault_provider=\"fixture\"\n[agent]\nmax_turns=12\nsafety_judge=false\nauto_title=false\ninput_suggestions=false\nsmall_model_routing=false\nauto_dispatch_read_only=false\n[local_model]\nenabled=false\n[providers.fixture]\nprovider=\"openai-compatible\"\napi=\"chat-completions\"\napi_base=\"http://127.0.0.1:#{fixture.port}/v1\"\napi_key=\"fixture-only\"\nmodel=\"fixture-model\"\n"
    File.write(File.join(home, 'config.toml'), config, perm: 0o600)
    env = workflow_fixture_env(home).merge('WILLDEEP_WORKSPACE_CHECKPOINTS' => '0')
    web_pid = nil
    begin
      version, error, status = Open3.capture3(BIN, '--version')
      raise error unless status.success?
      report[:version] = version.strip
      report[:binary] = { path: File.realpath(BIN), sha256: Digest::SHA256.file(BIN).hexdigest }
      child_api(env, 'workspace.register', root: workspace, name: '完整子会话验收', access: 'full_access', provider_profile: 'fixture')
      capabilities = child_api(env, 'runtime.capabilities')
      raise 'child capabilities missing' unless capabilities.fetch('operations').include?('session.child.start')
      parent = child_api(env, 'session.create', workspace: workspace, title: '调查与证据核验', profile: 'fixture').fetch('id')
      turn = child_api(env, 'turn.submit', session_id: parent, turn_request_id: SecureRandom.uuid,
        prompt: 'CHILD_P0_PARENT PARENT_PRIVATE_SENTINEL: delegate the bounded evidence read.', origin_client: 'child-fixture')
      task = wait_for('model-created complete child conversation') { child_tasks(env, parent).first }
      child = task.fetch('child_session_id')
      wait_for('parent final while independent child continues') { child_api(env, 'session.get', id: parent)['status'] == 'idle' }
      raise 'child work finished before parent' if child_tasks(env, parent).first['result']
      fixture.release(:child)
      result = child_terminal(env, parent, task.fetch('assignment_id'))
      raise "child failed to read evidence: #{result}" unless result['status'] == 'completed' && result.dig('result', 'summary').include?('READ_ONLY_EVIDENCE_42')
      wait_for('durable parent result consumption') do
        current = child_api(env, 'session.child.task.get', parent_session_id: parent, assignment_id: task['assignment_id'])
        current.dig('result', 'consumed_at')
      end
      wait_for('parent automatic result turn completed') { child_api(env, 'session.get', id: parent)['status'] == 'idle' }
      child_session = child_api(env, 'session.get', id: child)
      raise 'child identity or read-only projection missing' unless child_session['parent_session_id'] == parent && child_session['delegation_read_only'] && child_session['started_by'].nil?
      raise 'child root Agent reused the parent root' if child_session['root_agent_id'] == child_api(env, 'session.get', id: parent)['root_agent_id']
      history = child_api(env, 'session.export', id: child).dig('core', 'messages')
      raise 'child assignment was forged as user authorization' if history.any? { |message| message['role'] == 'user' && message['source'] != 'hostInstruction' }
      raise 'fresh copied parent private history' if JSON.generate(history).include?('PARENT_PRIVATE_SENTINEL')
      parent_history = child_api(env, 'session.export', id: parent).dig('core', 'messages')
      raise 'host notification was not persisted' unless parent_history.any? { |message| message['source'] == 'hostInstruction' && message['content'].include?('<host-context') }
      checks << { name: '模型派生、fresh 隔离、实际读文件、独立 root、父空闲回报与持久消费', passed: true, parent: parent, child: child, turn: turn['id'] }

      identity = SecureRandom.uuid
      request = { parent_session_id: parent, title: '幂等调查', prompt: 'CHILD_P0_READ', notify: 'none' }
      replayed = 4.times.map { Thread.new { child_api(env, 'session.child.start', request, request_id: identity) } }.map(&:value)
      raise 'concurrent replay created duplicate identities' unless replayed.map { |item| [item['assignment_id'], item['child_session_id']] }.uniq.size == 1
      child_refused(env, 'session.child.start', request.merge(prompt: 'changed request'), request_id: identity)
      replay_task = child_terminal(env, parent, replayed.first['assignment_id'])
      child_refused(env, 'session.child.start', request.merge(parent_session_id: child))
      stranger = child_api(env, 'session.create', workspace: workspace, title: '无关会话', profile: 'fixture').fetch('id')
      child_refused(env, 'session.child.task.get', parent_session_id: stranger, assignment_id: replay_task['assignment_id'])
      child_refused(env, 'session.child.continue', parent_session_id: stranger, child_session_id: child, prompt: 'unauthorized')
      child_refused(env, 'session.child.start', request.merge(context: 'fork'))
      child_refused(env, 'session.child.start', request.merge(workspace_mode: 'worktree'))
      checks << { name: '并发幂等、异参冲突、关系归属、单层派生及未支持模式拒绝', passed: true }

      continued = child_api(env, 'session.child.continue', parent_session_id: parent, child_session_id: child, prompt: 'CHILD_P0_READ: continue within the same history.', notify: 'none')
      continued = child_terminal(env, parent, continued['assignment_id'])
      raise 'continue changed child identity or reused assignment' unless continued['child_session_id'] == child && continued['assignment_id'] != task['assignment_id']
      child_refused(env, 'session.child.retry', parent_session_id: parent, assignment_id: continued['assignment_id'], expected_result_revision: continued.dig('result', 'revision'))
      checks << { name: '续做保留会话并创建新任务、成功任务不能冒充失败重试', passed: true }

      cli_task = child_cli(env, 'start', '--parent', parent, '--title', 'CLI 入口验收', 'CHILD_P0_READ')
      cli_finished = child_terminal(env, parent, cli_task['assignment_id'])
      raise 'CLI-created child failed' unless cli_finished['status'] == 'completed'
      cli_page = child_cli(env, 'list', '--parent', parent)
      raise 'CLI list omitted assignment' unless cli_page.fetch('tasks').any? { |item| item['assignment_id'] == cli_task['assignment_id'] }
      cli_get = child_cli(env, 'get', '--parent', parent, cli_task['assignment_id'])
      raise 'CLI get changed identity' unless cli_get['child_session_id'] == cli_task['child_session_id']
      cli_continued = child_cli(env, 'continue', '--parent', parent, cli_task['child_session_id'], 'CHILD_P0_READ CLI follow-up')
      cli_done = child_terminal(env, parent, cli_continued['assignment_id'])
      raise 'CLI continuation changed conversation or failed' unless cli_done['child_session_id'] == cli_task['child_session_id'] && cli_done['status'] == 'completed'
      cli_stopped = child_cli(env, 'stop', '--parent', parent, cli_done['assignment_id'])
      raise 'CLI terminal stop was not idempotent' unless cli_stopped['status'] == 'completed'
      checks << { name: '真实 CLI 创建、列表、查询、续做及终态停止入口', passed: true }

      %w[run_command create_file spawn_agent call_mcp_tool start_session].each do |name|
        probe = child_api(env, 'session.child.start', parent_session_id: parent, title: "只读边界核验 #{name}",
          prompt: "CHILD_P0_READ ATTEMPT_EFFECTS_#{name}", notify: 'none')
        probe = child_terminal(env, parent, probe['assignment_id'])
        raise "unexpected denial outcome for #{name}: #{probe}" unless %w[completed partial failed].include?(probe['status'])
        probe_history = child_api(env, 'session.export', id: probe['child_session_id']).dig('core', 'messages')
        calls = probe_history.flat_map { |message| message.fetch('tool_calls', []) }.select { |call| call['name'] == name }
        raise "fixture never attempted forbidden tool #{name}" if calls.empty?
        blocked = probe_history.any? do |message|
          message['role'] == 'tool' && calls.any? { |call| call['id'] == message['tool_call_id'] } &&
            message['content'].include?("read-only Workspace policy blocks tool: #{name}")
        end
        raise "missing execution-boundary refusal for #{name}" unless blocked
        raise 'child side effect escaped read-only cap' if File.exist?(File.join(workspace, 'child-forbidden'))
        raise 'child changed existing evidence' unless File.read(File.join(workspace, 'evidence.txt')) == "READ_ONLY_EVIDENCE_42\n"
        child_refused(env, 'agent.spawn', session_id: probe['child_session_id'], profile: 'generalist', prompt: 'must be rejected')
      end
      child_refused(env, 'turn.submit', session_id: parent, turn_request_id: SecureRandom.uuid,
        prompt: 'forged origin', origin_client: 'child-result-wake:forged')
      checks << { name: '实际执行边界拒绝 Shell、写文件、Worker、MCP、递归与伪造宿主来源', passed: true }

      quota_parents = 2.times.map do |index|
        child_api(env, 'session.create', workspace: workspace, title: "容量核验 #{index}", profile: 'fixture').fetch('id')
      end
      capacity_tasks = []
      20.times do |index|
        owner = quota_parents[index % quota_parents.size]
        capacity_tasks << child_api(env, 'session.child.start', parent_session_id: owner,
          title: "排队调查 #{index}", prompt: "CHILD_P0_READ CAP_CHILD #{index}", notify: 'none')
        if index == 3
          wait_for('four globally active child requests') do
            fixture.observations.count { |entry| entry[:child] && entry[:text].include?('CAP_CHILD') } == 4
          end
        end
      end
      child_refused(env, 'session.child.start', parent_session_id: quota_parents.first,
        title: '容量已满', prompt: 'CHILD_P0_READ CAP_CHILD overflow', notify: 'none')
      raise 'global child execution slots were exceeded' unless fixture.observations.count { |entry| entry[:child] && entry[:text].include?('CAP_CHILD') } == 4
      fixture.release(:capacity)
      capacity_tasks.each do |entry|
        finished = child_terminal(env, entry['parent_session_id'], entry['assignment_id'])
        raise "queued child did not complete: #{finished}" unless finished['status'] == 'completed'
      end
      checks << { name: '两个父会话共享宿主四执行槽、十六排队、拒绝溢出并逐项释放', passed: true, assignments: capacity_tasks.size }

      stopped_parent = child_api(env, 'session.create', workspace: workspace, title: '停止后的待处理结果', profile: 'fixture').fetch('id')
      stopped_turn = child_api(env, 'turn.submit', session_id: stopped_parent, turn_request_id: SecureRandom.uuid,
        prompt: 'CHILD_P0_PARENT STOP_SCENARIO: delegate the independent read.', origin_client: 'child-fixture')
      stopped_task = wait_for('child accepted before parent stop') { child_tasks(env, stopped_parent).first }
      wait_for('child model request before stop') { fixture.observations.any? { |entry| entry[:child] && entry[:text].include?('STOP_CHILD') } }
      child_api(env, 'turn.stop', id: stopped_turn.fetch('id'))
      fixture.release(:parent); fixture.release(:stopped_child)
      stopped_result = child_terminal(env, stopped_parent, stopped_task['assignment_id'])
      raise 'parent stop cancelled the independent child' unless stopped_result['status'] == 'completed'
      before = child_api(env, 'turn.list', session_id: stopped_parent)
      sleep 2 # Outbox reconciliation is asynchronous; verify no automatic parent Turn appears.
      after = child_api(env, 'turn.list', session_id: stopped_parent)
      raise 'stopped parent automatically resumed' unless before.map { |item| item['id'] } == after.map { |item| item['id'] }
      raise 'stopped result disappeared' if child_api(env, 'session.child.task.get', parent_session_id: stopped_parent, assignment_id: stopped_task['assignment_id']).dig('result', 'consumed_at')
      checks << { name: '主动停止父会话不取消已受理子任务、结果待处理且不自动拉起', passed: true }
      stopped_snapshot = child_api(env, 'session.child.task.get', parent_session_id: stopped_parent, assignment_id: stopped_task['assignment_id'])
      requests_before_restart = fixture.requests.size
      %w[stop start].each do |action|
        out, err, ok = Open3.capture3(env, BIN, 'daemon', action)
        raise "isolated daemon #{action} failed: #{err} #{out}" unless ok.success?
      end
      recovered = child_api(env, 'session.child.task.get', parent_session_id: stopped_parent, assignment_id: stopped_task['assignment_id'])
      raise 'restart changed durable result identity' unless recovered['result'] == stopped_snapshot['result']
      sleep 2
      raise 'restart replayed model execution for a stopped parent' unless fixture.requests.size == requests_before_restart
      checks << { name: '同目录重启保留任务结果和停止抑制、不重复执行模型', passed: true }

      # ---- P1：fork 只继承已完成 Turn 的不可变前缀 -------------------------
      fork_parent = child_api(env, 'session.create', workspace: workspace, title: '分叉前缀核验', profile: 'fixture').fetch('id')
      fork_turn_a = child_api(env, 'turn.submit', session_id: fork_parent, turn_request_id: SecureRandom.uuid,
        prompt: 'FORK_TURN_A_MARKER: first completed turn.',
        attachments: [{ kind: 'text', name: 'fork-note.txt', content: 'FORK_ATTACHMENT_CONTENT' }],
        origin_client: 'child-fixture')
      wait_for('fork boundary Turn completed') do
        child_api(env, 'turn.list', session_id: fork_parent).find { |item| item['id'] == fork_turn_a['id'] && item['status'] == 'completed' }
      end
      fork_turn_b = child_api(env, 'turn.submit', session_id: fork_parent, turn_request_id: SecureRandom.uuid,
        prompt: 'FORK_TURN_B_MARKER: must not travel into the fork.', origin_client: 'child-fixture')
      wait_for('later Turn completed') do
        child_api(env, 'turn.list', session_id: fork_parent).find { |item| item['id'] == fork_turn_b['id'] && item['status'] == 'completed' }
      end
      boundary = child_api(env, 'turn.list', session_id: fork_parent).find { |item| item['id'] == fork_turn_a['id'] }
      raise 'fork boundary has no durable message end' unless boundary['message_end'].is_a?(Integer) && boundary['message_end'].positive?
      fork_task = child_api(env, 'session.child.start', parent_session_id: fork_parent,
        title: '继承已完成前缀', prompt: 'CHILD_P0_READ: report what the inherited prefix contains.',
        context: 'fork', through_turn_id: boundary['id'], notify: 'none')
      fork_done = child_terminal(env, fork_parent, fork_task['assignment_id'])
      raise "fork child failed: #{fork_done}" unless fork_done['status'] == 'completed'
      raise 'fork task did not preserve its boundary' unless fork_done['context'] == 'fork' && fork_done['through_turn_id'] == boundary['id']
      fork_child = fork_done['child_session_id']
      parent_export = child_api(env, 'session.export', id: fork_parent)
      fork_export = child_api(env, 'session.export', id: fork_child)
      parent_prefix = parent_export.dig('core', 'messages')[0, boundary['message_end']]
      fork_messages = fork_export.dig('core', 'messages')
      fork_prefix = fork_messages[0, boundary['message_end']]
      raise 'fork history is shorter than its boundary' unless fork_messages.size >= boundary['message_end']
      raise 'fork prefix lost the copied system message' unless parent_prefix.first&.dig('role') == 'system' && fork_prefix.first&.dig('role') == 'system'
      unless fork_prefix[1..] == parent_prefix[1..]
        index = (1...[fork_prefix.size, parent_prefix.size].min).find { |position| fork_prefix[position] != parent_prefix[position] }
        raise "fork copied a mutable or later prefix (first mismatch at #{index.inspect}, parent=#{JSON.generate(parent_prefix[index]).slice(0, 600)} fork=#{JSON.generate(fork_prefix[index]).slice(0, 600)})"
      end
      # The system contract is execution configuration, not copied conversation:
      # the child must run under its own read-only ceiling.
      raise 'fork kept the parent system contract' unless fork_prefix.first['content'].include?('Delegated read-only conversation')
      raise 'fork lost a completed-turn attachment' unless JSON.generate(fork_prefix).include?('FORK_ATTACHMENT_CONTENT')
      raise 'fork copied the later Turn' if JSON.generate(fork_export.dig('core', 'messages')).include?('FORK_TURN_B_MARKER')
      raise 'fork changed the compression generation' unless fork_export.dig('core', 'compression_generation') == boundary['message_generation']
      raise 'fork inherited a compression checkpoint' unless fork_export.dig('core', 'compression_checkpoint').nil?
      fork_core = JSON.parse(File.read(File.join(home, 'sessions', "#{fork_child}.json"), encoding: Encoding::UTF_8))
      %w[current_plan goal goal_state].each do |field|
        raise "fork inherited #{field}" unless fork_core[field].nil?
      end
      %w[attention_read agent_reports_received agent_control_requests].each do |field|
        raise "fork inherited #{field}" unless fork_core[field].to_a.empty?
      end
      raise 'fork inherited the runtime event cursor' unless fork_core['runtime_event_cursor'].to_i.zero?
      raise 'fork inherited the pin' unless fork_core['pinned_at'].nil?
      if (checkpoint = fork_core['execution_checkpoint'])
        raise 'fork inherited pending tool calls' unless checkpoint['pending_call_ids'].to_a.empty?
        raise 'fork inherited worker report ids' unless checkpoint['agent_reports_received'].to_a.empty?
      end
      fork_session = child_api(env, 'session.get', id: fork_child)
      parent_root = child_api(env, 'session.get', id: fork_parent)['root_agent_id']
      raise 'fork child is not read-only' unless fork_session['delegation_read_only']
      raise 'fork child reused the parent root Agent' if fork_session['root_agent_id'] == parent_root
      checks << { name: 'fork 复制已完成前缀与内联附件、排除后续 Turn、清空执行状态并保持只读', passed: true,
        parent: fork_parent, child: fork_child, through_turn_id: boundary['id'], message_end: boundary['message_end'] }

      # ---- P1：独立可写 worktree（HEAD 与 snapshot 基线） -----------------
      child_git(workspace, 'init', '-q')
      child_git(workspace, 'config', 'user.name', 'WillDeep Fixture')
      child_git(workspace, 'config', 'user.email', 'fixture@example.invalid')
      File.write(File.join(workspace, 'test'), "exit 0\n")
      child_git(workspace, 'add', 'evidence.txt', 'test')
      child_git(workspace, 'commit', '-q', '-m', 'fixture baseline')
      File.write(File.join(workspace, 'tracked.txt'), "committed baseline\n")
      child_git(workspace, 'add', 'tracked.txt')
      child_git(workspace, 'commit', '-q', '-m', 'add tracked fixture')
      parent_head = child_git(workspace, 'rev-parse', 'HEAD').strip
      File.write(File.join(workspace, 'tracked.txt'), "dirty parent change\n")
      File.write(File.join(workspace, 'untracked.txt'), "parent untracked\n")

      worktree_parent = child_api(env, 'session.create', workspace: workspace, title: '工作树父会话', profile: 'fixture').fetch('id')
      child_api(env, 'turn.submit', session_id: worktree_parent, turn_request_id: SecureRandom.uuid,
        prompt: 'CHILD_P1_PARENT: delegate a writable worktree child from HEAD.', origin_client: 'child-fixture')
      head_task = wait_for('head worktree child accepted') { child_tasks(env, worktree_parent).first }
      raise 'head child was not a worktree task' unless head_task['workspace_mode'] == 'worktree'
      head_done = child_terminal(env, worktree_parent, head_task['assignment_id'])
      raise "head worktree child failed: #{head_done}" unless head_done['status'] == 'completed'
      child_settle(env, worktree_parent, head_done['assignment_id'])
      head_worktree = head_done.dig('result', 'worktree')
      raise 'head result lost its worktree binding' unless head_worktree
      raise 'head worktree used the wrong baseline kind' unless head_worktree['base'] == 'head'
      raise 'head worktree did not record the parent commit' unless head_worktree['base_commit'] == parent_head
      raise 'head worktree did not record parent dirtiness' unless head_worktree['parent_dirty']
      raise 'head worktree copied untracked files' unless head_worktree['untracked_files'].zero?
      raise 'head worktree branch prefix missing' unless head_worktree['branch'].start_with?('willdeep/child-')
      head_path = File.join(home, 'worktrees', 'children', head_done['child_session_id'])
      raise 'head worktree path escaped its deterministic location' unless File.realpath(head_worktree['path']) == File.realpath(head_path)
      raise 'head worktree is not a Git worktree' unless File.read(File.join(head_path, '.git')).start_with?('gitdir: ')
      head_session = child_api(env, 'session.get', id: head_done['child_session_id'])
      raise 'child Session did not move into its worktree' unless head_session['workspace'] == head_worktree['path']
      raise 'head child did not write its worktree' unless File.read(File.join(head_path, 'child-output.txt')) == 'CHILD_WROTE'
      raise 'head child wrote into the parent checkout' if File.exist?(File.join(workspace, 'child-output.txt'))
      raise 'head child imported untracked parent files' if File.exist?(File.join(head_path, 'untracked.txt'))
      raise 'head child imported the dirty parent file' unless File.read(File.join(head_path, 'tracked.txt')) == "committed baseline\n"
      raise 'parent checkout was modified by the child' unless File.read(File.join(workspace, 'tracked.txt')) == "dirty parent change\n"
      raise 'parent HEAD moved' unless child_git(workspace, 'rev-parse', 'HEAD').strip == parent_head
      raise 'head child did not report its real evidence' unless head_done.dig('result', 'summary').include?('CHILD_REPORT')
      head_history = child_api(env, 'session.export', id: head_done['child_session_id']).dig('core', 'messages')
      raise 'writable child never joined its Worker report' unless JSON.generate(head_history).include?('WORKER_REPORT')
      head_team = wait_for('family worker accounting') do
        summary = child_api(env, 'agent.team', id: head_session['root_agent_id'])
        summary if summary['workers'].any? { |worker| worker.dig('latest_run', 'status') == 'completed' }
      end
      raise 'child team did not bind the family root' unless head_team['family_session_id'] == worktree_parent
      raise 'family usage was not accounted' unless head_team.dig('family_usage', 'accounted_tokens').to_i.positive?
      parent_team = child_api(env, 'agent.team', id: child_api(env, 'session.get', id: worktree_parent)['root_agent_id'])
      raise 'parent team did not share the family usage' unless parent_team['family_usage'] == head_team['family_usage']

      snapshot_parent = child_api(env, 'session.create', workspace: workspace, title: '快照基线核验', profile: 'fixture').fetch('id')
      child_api(env, 'turn.submit', session_id: snapshot_parent, turn_request_id: SecureRandom.uuid,
        prompt: 'CHILD_P1_PARENT CHILD_P1_SNAPSHOT: freeze the parent tracked diff and untracked files.', origin_client: 'child-fixture')
      snapshot_task = wait_for('snapshot worktree child accepted') { child_tasks(env, snapshot_parent).first }
      snapshot_done = child_terminal(env, snapshot_parent, snapshot_task['assignment_id'])
      raise "snapshot worktree child failed: #{snapshot_done}" unless snapshot_done['status'] == 'completed'
      child_settle(env, snapshot_parent, snapshot_done['assignment_id'])
      snapshot_worktree = snapshot_done.dig('result', 'worktree')
      raise 'snapshot result lost its worktree binding' unless snapshot_worktree
      raise 'snapshot worktree used the wrong baseline kind' unless snapshot_worktree['base'] == 'snapshot'
      raise 'snapshot worktree did not record the parent commit' unless snapshot_worktree['base_commit'] == parent_head
      raise 'snapshot worktree has no frozen patch digest' unless snapshot_worktree['baseline_patch_sha256'].to_s.match?(/\A[0-9a-f]{64}\z/)
      raise 'snapshot worktree did not copy the untracked file' unless snapshot_worktree['untracked_files'] == 1
      raise 'snapshot worktree path collided with the head worktree' if snapshot_worktree['path'] == head_worktree['path']
      raise 'snapshot branch collided with the head branch' if snapshot_worktree['branch'] == head_worktree['branch']
      snapshot_path = snapshot_worktree['path']
      raise 'snapshot missed the frozen tracked diff' unless File.read(File.join(snapshot_path, 'tracked.txt')) == "dirty parent change\n"
      raise 'snapshot missed the frozen untracked file' unless File.read(File.join(snapshot_path, 'untracked.txt')) == "parent untracked\n"
      raise 'snapshot child did not write its worktree' unless File.read(File.join(snapshot_path, 'child-output.txt')) == 'CHILD_WROTE'
      raise 'snapshot child changed the parent checkout' unless File.read(File.join(workspace, 'tracked.txt')) == "dirty parent change\n"
      checks << { name: 'worktree 在取得执行槽后创建、HEAD 与 snapshot 基线隔离、父工作区零改动、Worker 计入家族用量', passed: true,
        head_child: head_done['child_session_id'], head_branch: head_worktree['branch'],
        snapshot_child: snapshot_done['child_session_id'], snapshot_branch: snapshot_worktree['branch'] }

      # ---- P1：模型覆盖按目标模型解析真实窗口 ----------------------------
      override_task = child_api(env, 'session.child.start', parent_session_id: worktree_parent,
        title: '模型覆盖核验', prompt: 'CHILD_P0_READ: report the bounded evidence.',
        model: 'child-override', notify: 'none')
      override_done = child_terminal(env, worktree_parent, override_task['assignment_id'])
      raise "model override child failed: #{override_done}" unless override_done['status'] == 'completed'
      raise 'task lost the model override' unless override_done['model'] == 'child-override'
      raise 'result lost the model override' unless override_done.dig('result', 'model') == 'child-override'
      override_session = child_api(env, 'session.get', id: override_done['child_session_id'])
      raise 'model override did not resolve the target window' unless override_session['model_context_window'] == 64_000
      raise 'fixture never served the override model' unless fixture.observations.any? { |entry| entry[:child] && entry[:model] == 'child-override' && entry[:text].include?('CHILD_P0_READ') }
      child_refused(env, 'session.child.start', parent_session_id: worktree_parent, title: '未知模型',
        prompt: 'CHILD_P0_READ', model: 'not-in-profile', notify: 'none')
      checks << { name: '模型覆盖只接受父 Profile 可用模型、按目标模型能力写入真实窗口并拒绝未知模型', passed: true,
        child: override_done['child_session_id'], model: 'child-override', context_window: override_session['model_context_window'] }

      report.merge!(passed: true, model_requests: fixture.requests.size, observations: fixture.observations.map { |entry| entry.reject { |key, _| key == :text } })
      child_report(output, report)
      puts JSON.pretty_generate(report.reject { |key, _| key == :observations })
      if options[:serve]
        log = File.open(File.join(root, 'web.log'), 'w')
        web_pid = Process.spawn(env, BIN, '--web', '--listen', "127.0.0.1:#{options[:port]}", '-w', workspace,
          '-c', env.fetch('WILLDEEP_CONFIG'), '--language', 'zh-CN', out: log, err: log)
        metadata = { root: root, home: home, workspace: workspace, parent: parent, child: child,
          worktree_parent: worktree_parent, worktree_child: head_done.fetch('child_session_id'),
          snapshot_parent: snapshot_parent, snapshot_child: snapshot_done.fetch('child_session_id'),
          stopped_parent: stopped_parent, port: options[:port], url: "http://127.0.0.1:#{options[:port]}" }
        File.write(File.join(output, 'fixture.json'), JSON.pretty_generate(metadata) + "\n")
        puts JSON.generate(metadata); $stdout.flush
        trap('TERM') { exit }; trap('INT') { exit }
        sleep 1 while true
      end
    rescue StandardError => error
      report.merge!(passed: false, error: "#{error.class}: #{error.message}")
      child_report(output, report)
      raise
    ensure
      fixture.release(:child); fixture.release(:stopped_child); fixture.release(:parent); fixture.release(:capacity)
      workflow_finish_child(web_pid)
      Open3.capture3(env, BIN, 'daemon', 'stop')
      fixture.close
    end
  end
end
