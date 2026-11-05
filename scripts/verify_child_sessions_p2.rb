#!/usr/bin/env ruby
# Real Runtime, synthetic state, deterministic loopback model, no personal Provider config.
require_relative 'verify_child_sessions'

class ChildSessionP2Fixture < ChildSessionModelFixture
  def initialize
    @p2_seen = []
    super
  end

  def seen_gate?(name)
    @mutex.synchronize { @p2_seen.include?(name.to_sym) }
  end

  def completion(request)
    text = JSON.generate(request.fetch('messages', []))
    text.scan(/P2_GATE_([a-z_]+)/).flatten.uniq.each do |name|
      key = name.to_sym
      @mutex.synchronize { @p2_seen << key unless @p2_seen.include?(key) }
      gate(key)
    end
    super
  end
end

def p2_link(env, parent, child)
  child_api(env, 'session.child.link.get', parent_session_id: parent, child_session_id: child)
end

def p2_start(env, parent, title, **options)
  child_api(env, 'session.child.start', { parent_session_id: parent, title: title,
    prompt: "CHILD_P0_READ #{title}", subscription: 'none' }.merge(options))
end

def p2_result_payload(task)
  task.fetch('result').reject { |key, _| %w[consumed_at superseded_by].include?(key) }
end

def p2_report(output, report)
  FileUtils.mkdir_p(output)
  File.write(File.join(output, 'report.json'), JSON.pretty_generate(report) + "\n")
  entries = report.fetch(:checks).map { |entry| "- #{entry[:name]}：通过" }
  text = "# 完整子会话 P2 隔离验收\n\n版本：#{report[:version]}\n\n结果：#{report[:passed] ? '通过' : '失败'}。真实 Runtime、临时 WILLDEEP_HOME、回环确定性模型，无远端付费调用。\n\n#{entries.join("\n")}\n"
  text += "\n错误：#{report[:error]}\n" if report[:error]
  File.write(File.join(output, 'report.md'), text)
end

if $PROGRAM_NAME == __FILE__
  options = { output: File.join(ROOT, 'build/verification/child-sessions-p2') }
  OptionParser.new do |parser|
    parser.on('--output DIR') { |directory| options[:output] = File.expand_path(directory) }
  end.parse!
  fixture, checks = ChildSessionP2Fixture.new, []
  report = { passed: false, checks: checks, loopback_only: true }
  Dir.mktmpdir('wd-child-sessions-p2-', '/private/tmp') do |root|
    home, workspace = File.join(root, 'home'), File.join(root, 'workspace')
    FileUtils.mkdir_p([home, workspace], mode: 0o700)
    File.write(File.join(workspace, 'evidence.txt'), "READ_ONLY_EVIDENCE_42\n")
    config = <<~CONFIG
      version=1
      default_provider="fixture"
      [agent]
      max_turns=12
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
    begin
      version, error, status = Open3.capture3(BIN, '--version')
      raise error unless status.success?
      report[:version] = version.strip
      report[:binary] = { path: File.realpath(BIN), sha256: Digest::SHA256.file(BIN).hexdigest }
      child_api(env, 'workspace.register', root: workspace, name: 'P2 子会话验收', access: 'workspace_write', provider_profile: 'fixture')
      capabilities = child_api(env, 'runtime.capabilities')
      %w[session_children_control session_children_subscriptions session_children_dependencies].each do |capability|
        raise "capability missing: #{capability}" unless capabilities.fetch('capabilities').include?(capability)
      end
      parent = child_api(env, 'session.create', workspace: workspace, title: 'P2 协调', profile: 'fixture').fetch('id')
      stranger = child_api(env, 'session.create', workspace: workspace, title: '无关会话', profile: 'fixture').fetch('id')
      checks << { name: '真实 Runtime 声明控制、订阅和依赖能力' }

      first = p2_start(env, parent, '前置调查', prompt: 'CHILD_P0_READ P2_GATE_upstream')
      wait_for('upstream executing') { fixture.seen_gate?(:upstream) }
      second = p2_start(env, parent, '后续调查', depends_on: [first.fetch('assignment_id')], subscription: 'inbox')
      third = p2_start(env, parent, '汇总调查', depends_on: [first.fetch('assignment_id'), second.fetch('assignment_id')])
      [second, third].each do |task|
        raise "dependency task started early: #{task}" unless task['waiting_on_dependencies'] && task['child_turn_id'].nil?
      end
      child_refused(env, 'session.child.start', parent_session_id: stranger, title: '跨父依赖', prompt: 'CHILD_P0_READ', depends_on: [first['assignment_id']])
      child_refused(env, 'session.child.start', parent_session_id: parent, title: '未知依赖', prompt: 'CHILD_P0_READ', depends_on: [SecureRandom.uuid])
      child_refused(env, 'session.child.start', parent_session_id: parent, title: '重复依赖', prompt: 'CHILD_P0_READ', depends_on: [first['assignment_id'], first['assignment_id']])
      fixture.release(:upstream)
      [first, second, third].each do |task|
        finished = child_terminal(env, parent, task['assignment_id'])
        raise "dependency chain failed: #{finished}" unless finished['status'] == 'completed'
      end
      checks << { name: '串行与多前置汇合只在成功后启动，等待无 Turn，拒绝跨父、未知及重复引用' }

      inbox = child_api(env, 'session.child.task.get', parent_session_id: parent, assignment_id: second['assignment_id'])
      notice = inbox.fetch('result').fetch('notification_id')
      payload = p2_result_payload(inbox)
      raise 'inbox-only result was consumed automatically' if inbox.dig('result', 'consumed_at')
      request = SecureRandom.uuid
      params = { parent_session_id: parent, assignment_id: second['assignment_id'],
        expected_subscription_revision: inbox.fetch('subscription_revision'), subscription: 'auto_wake' }
      replayed = 3.times.map { Thread.new { child_api(env, 'session.child.task.subscription.set', params, request_id: request) } }.map(&:value)
      raise 'subscription replay changed revision' unless replayed.map { |item| item['subscription_revision'] }.uniq.size == 1
      child_settle(env, parent, second['assignment_id'])
      consumed = child_api(env, 'session.child.task.get', parent_session_id: parent, assignment_id: second['assignment_id'])
      raise 'subscription changed notification identity or result' unless consumed.dig('result', 'notification_id') == notice && p2_result_payload(consumed) == payload
      child_refused(env, 'session.child.task.subscription.set', params.merge(subscription: 'none'))
      checks << { name: '仅列表结果按明确订阅补投，重放不重复变更，通知身份与终态报告不变' }

      failed = p2_start(env, parent, '取消前置', prompt: 'CHILD_P0_READ P2_GATE_cancelled_upstream')
      wait_for('cancellable predecessor running') { fixture.seen_gate?(:cancelled_upstream) }
      downstream = p2_start(env, parent, '失败后继', depends_on: [failed['assignment_id']])
      child_api(env, 'session.child.task.stop', parent_session_id: parent, assignment_id: failed['assignment_id'])
      fixture.release(:cancelled_upstream)
      cancelled = child_terminal(env, parent, failed['assignment_id'])
      blocked = child_terminal(env, parent, downstream['assignment_id'])
      raise 'stopped predecessor did not cancel' unless cancelled['status'] == 'cancelled'
      raise 'failed dependency executed downstream' unless blocked['status'] == 'blocked' && blocked['child_turn_id'].nil? && blocked['error'].to_s.size.positive?
      checks << { name: '前置取消使后继明确阻塞，后继不创建 Turn 或伪造成功' }

      pending_root = p2_start(env, parent, '等待中的前置', prompt: 'CHILD_P0_READ P2_GATE_pending_stop')
      wait_for('pending stop predecessor running') { fixture.seen_gate?(:pending_stop) }
      pending = p2_start(env, parent, '停止等待任务', depends_on: [pending_root['assignment_id']])
      stopped = child_api(env, 'session.child.task.stop', parent_session_id: parent, assignment_id: pending['assignment_id'])
      raise 'waiting task could not stop without a Turn' unless stopped['status'] == 'cancelled' && stopped['child_turn_id'].nil?
      fixture.release(:pending_stop)
      child_terminal(env, parent, pending_root['assignment_id'])
      checks << { name: '无 Turn 的依赖等待任务可以明确停止并释放等待容量' }

      owned = p2_start(env, parent, '用户接管', prompt: 'CHILD_P0_READ P2_GATE_takeover', subscription: 'auto_wake')
      wait_for('takeover target running') { fixture.seen_gate?(:takeover) }
      controlled_child = owned.fetch('child_session_id')
      waiting = p2_start(env, parent, '控制撤销后继', depends_on: [owned['assignment_id']])
      before_turns = child_api(env, 'turn.list', session_id: parent).map { |turn| turn['id'] }
      link = p2_link(env, parent, controlled_child)
      control_id = SecureRandom.uuid
      control_params = { parent_session_id: parent, child_session_id: controlled_child, expected_control_revision: link.fetch('control_revision') }
      owned_links = 3.times.map { Thread.new { child_api(env, 'session.child.takeover', control_params, request_id: control_id) } }.map(&:value)
      raise 'concurrent takeover replay created distinct revisions' unless owned_links.map { |item| item['control_revision'] }.uniq.size == 1
      raise 'takeover state missing' unless owned_links.first['control_state'] == 'user_owned'
      child_refused(env, 'session.child.detach', control_params)
      child_refused(env, 'session.child.continue', parent_session_id: parent, child_session_id: controlled_child, prompt: 'CHILD_P0_READ')
      child_refused(env, 'session.child.task.stop', parent_session_id: parent, assignment_id: owned['assignment_id'])
      child_refused(env, 'session.child.start', parent_session_id: parent, title: '接管后新依赖', prompt: 'CHILD_P0_READ', depends_on: [owned['assignment_id']])
      blocked_control = child_terminal(env, parent, waiting['assignment_id'])
      raise 'revoked-control dependency did not block' unless blocked_control['status'] == 'blocked' && blocked_control['child_turn_id'].nil?
      fixture.release(:takeover)
      finished_owned = child_terminal(env, parent, owned['assignment_id'])
      raise 'takeover implicitly cancelled existing execution' unless finished_owned['status'] == 'completed'
      raise 'takeover allowed automatic result consumption' if finished_owned.dig('result', 'consumed_at')
      after_turns = child_api(env, 'turn.list', session_id: parent).map { |turn| turn['id'] }
      raise 'takeover created an automatic parent model Turn' unless before_turns == after_turns
      checks << { name: '运行中接管不取消执行；并发重放幂等，撤销父控制与自动通知并阻塞待派发后继' }

      owned_link = p2_link(env, parent, controlled_child)
      detached = child_api(env, 'session.child.detach', parent_session_id: parent, child_session_id: controlled_child,
        expected_control_revision: owned_link.fetch('control_revision'))
      raise 'detach state missing' unless detached['control_state'] == 'detached'
      child_session = child_api(env, 'session.get', id: controlled_child)
      raise 'detach lost audit parent or read-only cap' unless child_session['parent_session_id'] == parent && child_session['delegation_read_only'] && child_session['delegation_detached']
      history = child_api(env, 'session.export', id: controlled_child)
      raise 'detach erased chat history' if history.dig('core', 'messages').empty?
      original_result = p2_result_payload(child_api(env, 'session.child.task.get', parent_session_id: parent, assignment_id: owned['assignment_id']))
      user_turn = child_api(env, 'turn.submit', session_id: controlled_child, turn_request_id: SecureRandom.uuid,
        prompt: 'CHILD_P0_READ ATTEMPT_EFFECTS_create_file: directly continue the detached conversation.', origin_client: 'p2-fixture')
      wait_for('detached conversation user turn finishes') { child_api(env, 'turn.get', id: user_turn['id'])['status'] == 'completed' }
      raise 'detached conversation bypassed persistent read-only cap' if File.exist?(File.join(workspace, 'child-forbidden'))
      raise 'direct child chat rewrote original assignment result' unless p2_result_payload(child_api(env, 'session.child.task.get', parent_session_id: parent, assignment_id: owned['assignment_id'])) == original_result
      checks << { name: '脱离保留来源、聊天和只读上限，普通后续聊天不重写旧委派结果' }

      cli_task = child_cli(env, 'start', '--parent', parent, '--title', 'P2 CLI 依赖任务',
        '--subscription', 'inbox', '--depends-on', third.fetch('assignment_id'), 'CHILD_P0_READ CLI_P2')
      cli_done = child_terminal(env, parent, cli_task.fetch('assignment_id'))
      raise 'CLI lost dependency or subscription' unless cli_done['depends_on'] == [third['assignment_id']] && cli_done['subscription'] == 'inbox'
      cli_changed = child_cli(env, 'subscription', 'set', '--parent', parent,
        '--expected-subscription-revision', cli_done.fetch('subscription_revision').to_s,
        '--subscription', 'none', cli_done.fetch('assignment_id'))
      raise 'CLI subscription set failed' unless cli_changed['subscription'] == 'none'
      cli_link = child_cli(env, 'link', '--parent', parent, cli_task.fetch('child_session_id'))
      output, error, confirmed = Open3.capture3(env, BIN, 'session', 'child', 'takeover', '--parent', parent,
        '--expected-control-revision', cli_link.fetch('control_revision').to_s, cli_task.fetch('child_session_id'))
      raise 'CLI changed control without explicit confirmation' if confirmed.success?
      raise 'CLI confirmation failure had no explanation' if (output + error).strip.empty?
      cli_taken = child_cli(env, 'takeover', '--parent', parent, '--yes',
        '--expected-control-revision', cli_link.fetch('control_revision').to_s, cli_task.fetch('child_session_id'))
      cli_detached = child_cli(env, 'detach', '--parent', parent, '--yes',
        '--expected-control-revision', cli_taken.fetch('control_revision').to_s, cli_task.fetch('child_session_id'))
      raise 'CLI detach failed' unless cli_detached['control_state'] == 'detached'
      checks << { name: '真实 CLI 依赖、订阅修改、关系查询与明确确认后的接管/脱离' }

      before_restart = child_api(env, 'turn.list', session_id: controlled_child).map { |turn| turn['id'] }
      child_stop_daemon(env)
      out, err, ok = Open3.capture3(env, BIN, 'daemon', 'start')
      raise "daemon start: #{err} #{out}" unless ok.success?
      restored = child_api(env, 'session.get', id: controlled_child)
      raise 'restart lost detached capability state' unless restored['delegation_detached'] && restored['delegation_read_only'] && restored['parent_session_id'] == parent
      raise 'restart lost control state' unless p2_link(env, parent, controlled_child)['control_state'] == 'detached'
      raise 'restart replayed a child Turn' unless child_api(env, 'turn.list', session_id: controlled_child).map { |turn| turn['id'] } == before_restart
      checks << { name: '同目录重启保留脱离、权限和控制状态，不重复创建执行轮次' }

      report[:model_requests] = fixture.requests.size
    rescue StandardError => error
      report.merge!(passed: false, error: "#{error.class}: #{error.message}")
      p2_report(options.fetch(:output), report)
      raise
    ensure
      %i[upstream cancelled_upstream pending_stop takeover].each { |key| fixture.release(key) }
      begin
        child_stop_daemon(env)
      rescue StandardError => error
        report.merge!(passed: false, error: "cleanup: #{error.class}: #{error.message}")
        p2_report(options.fetch(:output), report)
        raise
      ensure
        fixture.close
      end
    end
  end
  report[:passed] = true
  p2_report(options.fetch(:output), report)
  puts JSON.pretty_generate(report)
end
