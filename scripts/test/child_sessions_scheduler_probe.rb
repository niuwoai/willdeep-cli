#!/usr/bin/env ruby
# Diagnostic companion: keep the original 60s criterion, then observe bounded recovery.
require_relative '../verify_child_sessions'

output = File.expand_path(ARGV.fetch(0, File.join(ROOT, 'build/verification/child-sessions-p2/p0-p1/scheduler-probe')))
FileUtils.mkdir_p(output)
ChildSessionDiagnostics.output = output
File.write(File.join(output, 'trace.jsonl'), '')
fixture = ChildSessionModelFixture.new
report = { passed: false, checks: [], loopback_only: true, diagnostic_only: true }
Dir.mktmpdir('wd-child-sessions-probe-', '/private/tmp') do |root|
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
    api_key="loopback-fixture-only"
    model="fixture-model"
  CONFIG
  File.write(File.join(home, 'config.toml'), config, perm: 0o600)
  env = workflow_fixture_env(home).merge('WILLDEEP_WORKSPACE_CHECKPOINTS' => '0',
    'GIT_CONFIG_NOSYSTEM' => '1', 'GIT_CONFIG_GLOBAL' => '/dev/null')
  daemon_pid = nil
  begin
    report[:version] = child_capture(env, BIN, '--version').first.strip
    report[:binary] = { path: File.realpath(BIN), sha256: Digest::SHA256.file(BIN).hexdigest }
    child_git(workspace, '-c', 'init.defaultBranch=fixture', 'init', '-q')
    child_api(env, 'workspace.register', root: workspace, name: 'Scheduler probe', access: 'full_access', provider_profile: 'fixture')
    daemon_pid = child_capture(env, BIN, 'daemon', 'status').first[/\bpid=(\d+)/, 1]&.to_i
    report[:fixture] = { root: root, daemon_pid: daemon_pid, model_port: fixture.port }
    baseline = child_api(env, 'session.create', workspace: workspace, title: 'Baseline', profile: 'fixture').fetch('id')
    turn = child_api(env, 'turn.submit', session_id: baseline, turn_request_id: SecureRandom.uuid,
      prompt: 'SCHEDULER_BASELINE', origin_client: 'child-fixture')
    wait_for('baseline Turn completed') { child_api(env, 'turn.get', id: turn['id'])['status'] == 'completed' }
    report[:checks] << { name: '容量前普通父 Turn 可启动并完成', passed: true }

    parents = 2.times.map { child_api(env, 'session.create', workspace: workspace, title: 'Capacity', profile: 'fixture').fetch('id') }
    tasks = 20.times.map do |index|
      task = child_api(env, 'session.child.start', parent_session_id: parents[index % 2],
        title: "Capacity #{index}", prompt: "CHILD_P0_READ CAP_CHILD #{index}", notify: 'none')
      if index == 3
        wait_for('four globally active child requests') do
          fixture.observations.count { |entry| entry[:child] && entry[:text].include?('CAP_CHILD') } == 4
        end
      end
      task
    end
    child_refused(env, 'session.child.start', parent_session_id: parents.first,
      title: 'Overflow', prompt: 'CHILD_P0_READ CAP_CHILD overflow', notify: 'none')
    fixture.release(:capacity)
    tasks.each do |task|
      done = child_terminal(env, task['parent_session_id'], task['assignment_id'])
      raise "capacity task failed: #{done}" unless done['status'] == 'completed'
    end
    report[:checks] << { name: '四执行槽与十六排队全部完成', passed: true }
    parent = child_api(env, 'session.create', workspace: workspace, title: 'Post-capacity parent', profile: 'fixture').fetch('id')
    turn = child_api(env, 'turn.submit', session_id: parent, turn_request_id: SecureRandom.uuid,
      prompt: 'CHILD_P0_PARENT STOP_SCENARIO: delegate the independent read.', origin_client: 'child-fixture')
    report[:parent], report[:turn] = parent, turn['id']
    started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    begin
      task = wait_for('child accepted before parent stop') { child_tasks(env, parent).first }
      report[:passed] = true
    rescue StandardError => error
      report[:error] = "#{error.class}: #{error.message}"
      report[:failure_evidence] = child_failure_evidence(output, home, fixture, daemon_pid)
      # This extra window never turns the failed 60s check into a pass.
      deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + 90
      loop do
        task = child_tasks(env, parent).first
        break if task || Process.clock_gettime(Process::CLOCK_MONOTONIC) >= deadline
        sleep 1
      end
    end
    report[:accepted_after_seconds] = Process.clock_gettime(Process::CLOCK_MONOTONIC) - started if task
    report[:eventually_accepted] = !task.nil?
    report[:turn_after_observation] = child_api(env, 'turn.get', id: turn['id'])
    report[:stop_model_requests] = fixture.observations.count { |entry| entry[:text].include?('STOP_SCENARIO') }
    report[:checks] << { name: '容量后新父 Turn 在原有 60 秒内派生子任务', passed: report[:passed] }
    report[:observation_evidence] = child_failure_evidence(output, home, fixture, daemon_pid) unless report[:passed]
    unless task
      # Restart only this synthetic Daemon, preserving its durable ledger. This
      # distinguishes an in-memory scheduling backlog from a stuck model call.
      child_stop_daemon(env)
      out, err, status = child_capture(env, BIN, 'daemon', 'start')
      raise "isolated diagnostic restart failed: #{err} #{out}" unless status.success?
      daemon_pid = child_capture(env, BIN, 'daemon', 'status').first[/\bpid=(\d+)/, 1]&.to_i
      restarted = Process.clock_gettime(Process::CLOCK_MONOTONIC)
      begin
        recovered = wait_for('diagnostic same Turn dispatch after isolated restart', 30) { child_tasks(env, parent).first }
        report[:accepted_after_restart_seconds] = Process.clock_gettime(Process::CLOCK_MONOTONIC) - restarted
        report[:recovered_assignment] = recovered['assignment_id']
      rescue StandardError => error
        report[:restart_observation_error] = "#{error.class}: #{error.message}"
      end
    end
  rescue StandardError => error
    report[:passed] = false
    report[:error] ||= "#{error.class}: #{error.message}"
    report[:failure_evidence] ||= child_failure_evidence(output, home, fixture, daemon_pid)
  ensure
    fixture.release(:capacity); fixture.release(:parent); fixture.release(:stopped_child); fixture.release(:child)
    begin
      child_stop_daemon(env)
    rescue StandardError => error
      report[:passed] = false
      report[:cleanup_error] = "#{error.class}: #{error.message}"
    ensure
      fixture.close
      child_report(output, report)
    end
  end
end
puts JSON.pretty_generate(report)
exit(report[:passed] ? 0 : 1)
