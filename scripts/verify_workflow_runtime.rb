#!/usr/bin/env ruby
# Real isolated Runtime + deterministic loopback model. --serve also exposes its Web UI.
require_relative 'verify_session_collaboration'
require_relative 'verify_workflow_tui'
require 'optparse'
require 'rbconfig'

class WorkflowModelFixture < ModelFixture
  def initialize
    @lock, @condition = Mutex.new, ConditionVariable.new
    @observations, @events, @active, @counts, @finished = [], [], {}, Hash.new(0), Hash.new(0)
    @released, @max_active, @overlap, @failed = {}, 0, false, false
    super
  end

  def observations = @lock.synchronize { @observations.map(&:dup) }
  def telemetry
    @lock.synchronize { { counts: @counts.dup, finished: @finished.dup, active: @active.dup,
      max_active: @max_active, overlap: @overlap, events: @events.map(&:dup) } }
  end
  def release(gate)
    @lock.synchronize { @released[gate] = true; @condition.broadcast }
  end

  def completion(request)
    messages = request.fetch('messages', [])
    text = JSON.generate(messages)
    parent = request.fetch('tools', []).any? { |t| t.dig('function', 'name') == 'agent_library_manage' }
    calls = messages.flat_map { |m| m.fetch('tool_calls', []) }
    names = calls.to_h { |c| [c.fetch('id'), c.dig('function', 'name')] }
    results = messages.select { |m| m['role'] == 'tool' && names[m['tool_call_id']] }.filter_map do |m|
      [names[m['tool_call_id']], JSON.parse(m.fetch('content'))]
    rescue JSON::ParserError
      nil
    end
    call, content = nil, 'Local fixture completed.'
    if parent && text.include?('CHAT_WORKFLOW_E2E')
      controls = calls.select { |c| c.dig('function', 'name') == 'workflow_control' }
      definition_id, leaf_prompt = text.include?('BROWSER_SLOW_LEAF') ? ['browser-pipeline', 'BROWSER_SLOW_LEAF'] : ['chat-pipeline', 'SLOW_WORKFLOW_LEAF']
      call = if results.empty?
        ['workflow_read', { operation: 'schema', params: {} }]
      elsif controls.empty?
        ['workflow_control', { operation: 'save', params: { spec: workflow_spec(definition_id, leaf_prompt) } }]
      elsif controls.size == 1
        ['workflow_control', { operation: 'run', params: { definition_id: definition_id, input: {} } }]
      end
      @lock.synchronize { @events << { event: 'chat_final', at: monotonic } } unless call
      content = 'CHAT_WORKFLOW_FINAL: workflow admitted; stages continue in the Runtime.'
    elsif parent && text.include?('CHAT_CONCURRENCY_E2E')
      unless calls.any? { |c| %w[spawn_agent workflow_control].include?(c.dig('function', 'name')) }
        @lock.synchronize { @events << { event: 'load_dispatch', at: monotonic } }
        call = [['spawn_agent', { profile: 'generalist', label: 'load-ordinary', prompt: 'WF_LOAD_ordinary', run_in_background: true }]] +
          %w[alpha beta].map { |id| ['workflow_control', { operation: 'run', params: { definition_id: "load-#{id}", limits: { max_concurrent: 2 } } }] }
      end
    elsif parent && text.include?('RECOVERY_LIVE_PROBE')
      call = if !calls.any? { |c| c.dig('function', 'name') == 'spawn_agent' }
        ['spawn_agent', { profile: 'user/recovery-fixture', prompt: 'RECOVERY_LIVE_PROBE_LEAF', run_in_background: true }]
      elsif !calls.any? { |c| c.dig('function', 'name') == 'await_agents' }
        ['await_agents', { timeout_seconds: 10 }]
      end
    elsif parent && text.include?('CHAT_LIBRARY_E2E')
      definition = JSON.parse(File.read(File.join(ROOT, 'docs/examples/agent-definition.v1.json')))
      definition.merge!('definition_id' => 'user/chat-fixture', 'instructions' => 'CHAT_DEFINITION_V1: report independently reproducible evidence.')
      managed = results.select { |name, _| name == 'agent_library_manage' }.map(&:last)
      spawned = calls.count { |c| c.dig('function', 'name') == 'spawn_agent' }
      raise "chat management failed: #{managed.last}" if managed.last && managed.last['saved'] != true
      call = if results.empty?
        ['agent_library_read', { operation: 'list', params: {} }]
      elsif managed.empty?
        ['agent_library_manage', { operation: 'create', params: { definition: definition } }]
      elsif spawned.zero?
        ['spawn_agent', { profile: definition['definition_id'], prompt: 'OLD_SNAPSHOT: inspect without writing.', run_in_background: true }]
      elsif managed.size == 1
        definition.merge!('version' => '1.0.0-rc2', 'instructions' => 'CHAT_DEFINITION_V2: use the updated review checklist.')
        ['agent_library_manage', { operation: 'update', params: { definition: definition, expected_revision: managed.last.fetch('installation_revision') } }]
      elsif managed.size == 2
        ['agent_library_manage', { operation: 'bind', params: { id: definition['definition_id'], binding: { model: 'fixture-model-v2' }, expected_revision: managed.last.fetch('installation_revision') } }]
      elsif spawned == 1
        ['spawn_agent', { profile: definition['definition_id'], prompt: 'NEW_SNAPSHOT: inspect without writing.', run_in_background: true }]
      elsif !calls.any? { |c| c.dig('function', 'name') == 'await_agents' }
        ['await_agents', { timeout_seconds: 10 }]
      end
      content = 'CHAT_LIBRARY_E2E completed: created, updated, bound and dispatched in one conversation.'
    elsif !parent
      tag = text[/WF_LOAD_(?:ordinary|(?:alpha|beta)_\d{2})|RECOVERY_(?:DONE|INFLIGHT|TAIL|LIVE_PROBE_LEAF)/]
      if text.include?('PIPELINE_INSPECT') || text.include?('PIPELINE_VERIFY')
        item = text.include?('PIPELINE_SLOW') ? 'slow' : 'fast'
        stage = text.include?('PIPELINE_VERIFY') ? 'verify' : 'inspect'
        tag = "PIPE_#{item}_#{stage}"
      end
      group = tag&.start_with?('WF_LOAD_') ? tag.sub('WF_LOAD_', '').split('_').first : nil
      @lock.synchronize do
        @observations << { model: request['model'], workflow: text.include?('Workflow result contract'), tag: tag,
          old: text.include?('OLD_SNAPSHOT'), new: text.include?('NEW_SNAPSHOT'), v1: text.include?('CHAT_DEFINITION_V1'), v2: text.include?('CHAT_DEFINITION_V2'),
          catalog_v1: text.include?('RECOVERY_CATALOG_V1'), catalog_v2: text.include?('RECOVERY_CATALOG_V2') }
        if tag
          @counts[tag] += 1; @active[tag] = group
          groups = @active.values.compact
          @max_active = [@max_active, groups.size].max
          @overlap ||= %w[ordinary alpha beta].all? { |g| groups.include?(g) }
          @events << { event: 'start', tag: tag, at: monotonic, active: @active.dup }
        end
      end
      begin
        gate = if tag == 'RECOVERY_INFLIGHT'
          :recovery
        elsif tag == 'PIPE_slow_inspect'
          :pipeline
        elsif tag && (tag == 'WF_LOAD_ordinary' || tag.end_with?('_00'))
          :load
        end
        @lock.synchronize do
          deadline = monotonic + 50
          @condition.wait(@lock, 0.2) until @released[gate] || monotonic > deadline
          raise "fixture gate timed out: #{gate}" unless @released[gate]
        end if gate
        sleep 1.5 if group
        sleep 2 if text.include?('OLD_SNAPSHOT')
        sleep 4 if text.include?('SLOW_WORKFLOW_LEAF')
        sleep 15 if text.include?('BROWSER_SLOW_LEAF')
        invalid = @lock.synchronize { first = text.include?('FAIL_FIRST_WORKFLOW_LEAF') && !@failed; @failed ||= first; first }
        content = if text.include?('Workflow result contract')
          invalid ? 'Deliberately invalid JSON for the first attempt.' : JSON.generate(ok: true, evidence: 'loopback fixture evidence', model: request['model'])
        else
          "CONCLUSION: local review completed.\nEVIDENCE: synthetic fixture.\nOPEN QUESTIONS: none."
        end
      ensure
        @lock.synchronize do
          if tag
            @active.delete(tag); @finished[tag] += 1
            @events << { event: 'finish', tag: tag, at: monotonic, active: @active.dup }
          end
        end
      end
    end
    message = { role: 'assistant', content: call ? nil : content }
    if call
      batch = call[0].is_a?(Array) ? call : [call]
      message[:tool_calls] = batch.map { |name, args| { id: "call_#{SecureRandom.hex(8)}", type: 'function', function: { name: name, arguments: JSON.generate(args) } } }
    end
    { id: "chatcmpl-#{SecureRandom.hex(8)}", object: 'chat.completion', model: request['model'],
      choices: [{ index: 0, message: message, finish_reason: call ? 'tool_calls' : 'stop' }], usage: { prompt_tokens: 100, completion_tokens: 20, total_tokens: 120 } }
  end

  def monotonic = Process.clock_gettime(Process::CLOCK_MONOTONIC)
end

def workflow_spec(id, prompt = 'Inspect the supplied evidence')
  { version: 1, id: id, name: '调查与核验', phases: [{ id: 'discover', name: '调查' }, { id: 'verify', name: '核验' }],
    nodes: [{ id: 'inspect', kind: 'agent', phase: 'discover', profile: 'generalist', prompt: prompt, output_schema: { type: 'object', required: ['ok'] } },
      { id: 'verify', kind: 'agent', phase: 'verify', depends_on: ['inspect'], profile: 'generalist', prompt: 'Verify the evidence', input: { '$ref' => 'node:inspect#' } }], output: { '$ref' => 'node:verify#' } }
end

def fixture_api(env, operation, params = {})
  arguments = [BIN, 'api', operation, '--params-file', '-', '--ndjson']
  identity = params[:request_id] || params['request_id']
  arguments.concat(['--request-id', identity]) if identity
  output, error, status = Open3.capture3(env, *arguments, stdin_data: JSON.generate(params))
  raise "#{operation}: #{error}\n#{output}" unless status.success?
  response = JSON.parse(output)
  raise "#{operation}: #{response}" unless response['status'] == 'ok'
  response.fetch('data')
end

def wait_workflow(env, parent, id)
  run = nil
  wait_for('workflow terminal') do
    run = fixture_api(env, 'workflow.get', parent_session_id: parent, run_id: id).fetch('workflow')
    run unless %w[running stopping].include?(run.fetch('status'))
  end
rescue RuntimeError => error
  session = fixture_api(env, 'session.get', id: parent)
  turns = fixture_api(env, 'turn.list', session_id: parent)
  raise "#{error.message}; last workflow=#{run}; session=#{session}; turns=#{turns}"
end

def library_mutation(env, operation, params)
  scope = fixture_api(env, 'agent_library.list').fetch('scope')
  unless operation == 'create'
    selected = fixture_api(env, 'agent_library.get', id: params[:id] || params.dig(:definition, 'definition_id'))
    params = params.merge(target_scope: selected.fetch('target_scope'))
  end
  fixture_api(env, "agent_library.#{operation}", params.merge(expected_scope: scope, request_id: SecureRandom.uuid))
end

def verify_recovery(env, fixture, root, workspace)
  parent = fixture_api(env, 'session.create', workspace: workspace, title: 'Crash recovery fixture', profile: 'fixture').fetch('id')
  definition = JSON.parse(File.read(File.join(ROOT, 'docs/examples/agent-definition.v1.json')))
  definition.merge!('definition_id' => 'user/recovery-fixture', 'instructions' => 'RECOVERY_CATALOG_V1')
  saved = library_mutation(env, 'create', definition: definition)
  library_mutation(env, 'bind', id: definition['definition_id'], binding: { model: 'fixture-recovery-v1' }, expected_revision: saved.fetch('installation_revision'))
  spec = { version: 1, id: 'recovery', name: 'Recovery fixture', phases: [{ id: 'checkpoint', name: 'Checkpoint' }, { id: 'resume', name: 'Resume phase' }],
    nodes: %w[checkpoint inflight tail].each_with_index.map { |id, i| { id: id, kind: 'agent', phase: i.zero? ? 'checkpoint' : 'resume', profile: definition['definition_id'],
      prompt: %w[RECOVERY_DONE RECOVERY_INFLIGHT RECOVERY_TAIL][i], depends_on: i.zero? ? [] : [%w[checkpoint inflight][i - 1]] } }, output: { '$ref' => 'node:tail#' } }
  fixture_api(env, 'workflow.save', parent_session_id: parent, spec: spec)
  run = fixture_api(env, 'workflow.run', parent_session_id: parent, definition_id: 'recovery', request_id: SecureRandom.uuid).fetch('workflow')
  before = wait_for('durable checkpoint and in-flight model request') do
    current = fixture_api(env, 'workflow.get', parent_session_id: parent, run_id: run['id']).fetch('workflow')
    current if current.dig('nodes', 0, 'status') == 'completed' && current.dig('nodes', 1, 'agent_run', 'status') == 'running' && fixture.telemetry[:active].key?('RECOVERY_INFLIGHT')
  end
  live = fixture_api(env, 'agent_library.get', id: definition['definition_id'])
  definition.merge!('version' => '1.0.0-rc2', 'instructions' => 'RECOVERY_CATALOG_V2')
  saved = library_mutation(env, 'update', definition: definition, expected_revision: live.fetch('installation_revision'))
  library_mutation(env, 'bind', id: definition['definition_id'], binding: { model: 'fixture-recovery-v2' }, expected_revision: saved.fetch('installation_revision'))
  changed = Marshal.load(Marshal.dump(spec)); changed[:nodes].each { |node| node[:prompt] = 'CHANGED_SAVED_WORKFLOW_MUST_NOT_RUN' }
  changed[:id] = 'recovery-updated'
  fixture_api(env, 'workflow.save', parent_session_id: parent, spec: changed)
  validate_workflow_fixture!({ 'root' => root, 'home' => env.fetch('WILLDEEP_HOME'), 'workspace' => workspace })
  status_text, error, status = Open3.capture3(env, BIN, 'daemon', 'status')
  pid = status_text[/\Arunning\tpid=(\d+)\t/, 1]&.to_i
  raise "isolated daemon status did not identify a running PID: #{error}" unless status.success? && pid && pid > 1 && pid != Process.pid
  Process.kill('KILL', pid) # Only this fresh fixture's daemon, identified by its normal CLI.
  wait_for('isolated daemon crash') do
    begin
      Process.kill(0, pid); false
    rescue Errno::ESRCH
      true
    end
  end
  fixture.release(:recovery)
  wait_for('abandoned model request drained') { !fixture.telemetry[:active].key?('RECOVERY_INFLIGHT') }
  out, err, started = Open3.capture3(env, BIN, 'daemon', 'start')
  raise "same-home daemon restart failed: #{err} #{out}" unless started.success?
  interrupted = wait_workflow(env, parent, run['id'])
  raise "in-flight execution was replayed or misclassified: #{interrupted}" unless interrupted['status'] == 'interrupted' && interrupted['nodes'][1]['status'] == 'interrupted' && interrupted['nodes'][1].dig('agent_run', 'usage', 'uncertain_execution')
  raise 'durable successful checkpoint changed on restart' unless interrupted['nodes'][0] == before['nodes'][0]
  wait_for('recovery aggregate committed') { fixture_api(env, 'session.get', id: parent)['status'] == 'idle' }
  counts = fixture.telemetry[:counts]
  raise "restart replayed work: #{counts}" unless counts.values_at('RECOVERY_DONE', 'RECOVERY_INFLIGHT') == [1, 1] && counts.fetch('RECOVERY_TAIL', 0).zero?
  resumed = fixture_api(env, 'workflow.resume', parent_session_id: parent, run_id: run['id'], request_id: SecureRandom.uuid).fetch('workflow')
  resumed = wait_workflow(env, parent, resumed['id'])
  reused = resumed['nodes'][0]
  raise 'resume did not create a linked, frozen successful run' unless resumed['status'] == 'completed' && resumed['id'] != run['id'] && resumed['source_run_id'] == run['id'] && resumed['spec'] == before['spec']
  raise 'resume did not reuse the exact completed output' unless reused['reused_from'] == { 'run_id' => run['id'], 'node_instance_id' => before['nodes'][0]['id'] } && reused['output'] == before['nodes'][0]['output'] && reused.dig('agent_run', 'id') == before['nodes'][0].dig('agent_run', 'id')
  raise 'resume reused uncertain execution identity' if resumed['nodes'][1].dig('agent_run', 'id') == before['nodes'][1].dig('agent_run', 'id')
  source_after = fixture_api(env, 'workflow.get', parent_session_id: parent, run_id: run['id']).fetch('workflow')
  raise 'resume mutated source history' unless source_after['status'] == 'interrupted' && source_after['nodes'] == interrupted['nodes']
  observations = fixture.observations.select { |entry| %w[RECOVERY_DONE RECOVERY_INFLIGHT RECOVERY_TAIL].include?(entry[:tag]) }
  raise 'resume lost frozen catalog/model routing' unless observations.size == 4 && observations.all? { |entry| entry[:catalog_v1] && !entry[:catalog_v2] && entry[:model] == 'fixture-recovery-v1' }
  raise 'successful node reran or uncertain node duplicated' unless fixture.telemetry[:counts].values_at('RECOVERY_DONE', 'RECOVERY_INFLIGHT', 'RECOVERY_TAIL') == [1, 2, 1]
  fixture_api(env, 'turn.submit', session_id: parent, turn_request_id: SecureRandom.uuid, prompt: 'RECOVERY_LIVE_PROBE', origin_client: 'workflow-fixture')
  wait_for('live catalog probe') { fixture_api(env, 'session.get', id: parent)['status'] == 'idle' && fixture.observations.any? { |entry| entry[:tag] == 'RECOVERY_LIVE_PROBE_LEAF' } }
  probe = fixture.observations.find { |entry| entry[:tag] == 'RECOVERY_LIVE_PROBE_LEAF' }
  raise 'updated live catalog was not demonstrably different' unless probe[:catalog_v2] && !probe[:catalog_v1] && probe[:model] == 'fixture-recovery-v2'
  { check: 'same_home_crash_interrupted_explicit_resume_reuses_success_and_frozen_catalog', passed: true, parent: parent, source: run['id'], run: resumed['id'], killed_fixture_pid: pid, observations: observations, live_probe: probe }
end

def verify_item_pipeline(env, fixture, workspace)
  parent = fixture_api(env, 'session.create', workspace: workspace, title: 'Item pipeline fixture', profile: 'fixture').fetch('id')
  spec = { version: 1, id: 'item-pipeline', name: 'Item pipeline', nodes: [
    { id: 'checks', kind: 'foreach', items: { '$ref' => 'input#/items' }, body: {
      nodes: [
        { id: 'inspect', kind: 'agent', profile: 'generalist', prompt: 'PIPELINE_INSPECT', input: { '$ref' => 'item#' } },
        { id: 'verify', kind: 'agent', profile: 'reviewer', prompt: 'PIPELINE_VERIFY', depends_on: ['inspect'],
          input: { item: { '$ref' => 'item#' }, evidence: { '$ref' => 'node:inspect#' } } }
      ], output: { '$ref' => 'node:verify#' }
    } },
    { id: 'summary', kind: 'agent', profile: 'generalist', prompt: 'Summarize the verified items as JSON.',
      depends_on: ['checks'], input: { '$ref' => 'node:checks#' } }
  ], output: { '$ref' => 'node:summary#' } }
  fixture_api(env, 'workflow.save', parent_session_id: parent, spec: spec)
  run = fixture_api(env, 'workflow.run', parent_session_id: parent, definition_id: spec[:id],
    input: { items: ['PIPELINE_FAST', 'PIPELINE_SLOW'] }, request_id: SecureRandom.uuid).fetch('workflow')
  independent = wait_for('fast item verified while slow inspection remains blocked') do
    telemetry = fixture.telemetry
    next unless telemetry[:finished]['PIPE_fast_verify'] == 1 && telemetry[:active].key?('PIPE_slow_inspect')
    current = fixture_api(env, 'workflow.get', parent_session_id: parent, run_id: run['id']).fetch('workflow')
    current if current['nodes'].any? { |node| node['path'] == 'checks/0/verify' && node['status'] == 'completed' }
  end
  statuses = independent['nodes'].to_h { |node| [node['path'], node['status']] }
  raise "item pipeline waited for the slow peer: #{statuses}" unless statuses['checks/0/verify'] == 'completed' && statuses['checks/1/inspect'] != 'completed'
  fixture.release(:pipeline)
  completed = wait_workflow(env, parent, run['id'])
  raise 'item pipeline did not collect and summarize all results' unless completed['status'] == 'completed' && completed['nodes'].all? { |node| node['status'] == 'completed' }
  counts = fixture.telemetry[:counts].select { |tag, _| tag.start_with?('PIPE_') }
  raise "item pipeline repeated a leaf: #{counts}" unless counts.size == 4 && counts.values.all? { |count| count == 1 }
  { check: 'parallel_items_verify_independently_before_summary', passed: true, parent: parent, run: run['id'],
    intermediate_statuses: statuses, model_counts: counts, events: fixture.telemetry[:events].select { |event| event[:tag]&.start_with?('PIPE_') } }
ensure
  fixture.release(:pipeline)
end

def verify_shared_scheduler(env, fixture, workspace)
  parent = fixture_api(env, 'session.create', workspace: workspace, title: 'Shared scheduler fixture', profile: 'fixture').fetch('id')
  %w[alpha beta].each do |group|
    spec = { version: 1, id: "load-#{group}", name: "Load #{group}", nodes: 9.times.map { |i| { id: "leaf#{i}", kind: 'agent', profile: 'generalist', prompt: format('WF_LOAD_%s_%02d', group, i) } } }
    fixture_api(env, 'workflow.save', parent_session_id: parent, spec: spec)
  end
  turn = fixture_api(env, 'turn.submit', session_id: parent, turn_request_id: SecureRandom.uuid, prompt: 'CHAT_CONCURRENCY_E2E', origin_client: 'workflow-fixture')
  wait_for('parent load dispatch') { fixture.telemetry[:events].any? { |e| e[:event] == 'load_dispatch' } }
  agent = fixture_api(env, 'session.get', id: parent).fetch('root_agent_id')
  wait_for('team policy persisted') { fixture_api(env, 'agent.team', id: agent)['policy'] }
  samples, saturated = [], false
  sample = lambda do
    team = fixture_api(env, 'agent.team', id: agent)
    raise "unexpected effective scheduler policy: #{team['policy']}" unless team.dig('policy', 'max_concurrent') == 3 && team.dig('policy', 'max_queued') == 2
    raise "shared scheduler exceeded its limit: #{team}" if team['running'] > 3 || team['queued'] > 2
    runs = fixture_api(env, 'workflow.list', parent_session_id: parent).fetch('runs')
    samples << { running: team['running'], queued: team['queued'], workflow_states: runs.map { |run| { id: run['id'], status: run['status'], nodes: run['nodes'].map { |n| n['status'] } } } }
    saturated ||= team['queued'] == 2 && runs.any? { |r| r['nodes'].any? { |n| %w[pending dispatch_pending].include?(n['status']) } }
    runs
  end
  wait_for('shared queue saturation and ordinary/alpha/beta simultaneously active') do
    sample.call
    saturated && fixture.telemetry[:overlap]
  end
  fixture.release(:load)
  complete = wait_for('all 18 workflow leaves and ordinary worker') do
    runs = sample.call
    team = fixture_api(env, 'agent.team', id: agent)
    runs if runs.size == 2 && runs.all? { |r| r['status'] == 'completed' } && team['running'].zero? && team['queued'].zero? && team['completed_runs'] == 19
  end
  ledger = fixture_api(env, 'agent.runs', id: agent, limit: 100).fetch('runs')
  expected = ['WF_LOAD_ordinary'] + %w[alpha beta].flat_map { |group| 9.times.map { |i| format('WF_LOAD_%s_%02d', group, i) } }
  telemetry = fixture.telemetry
  raise 'fixture did not prove the shared maximum and simultaneous activity' unless telemetry[:max_active] == 3 && telemetry[:overlap]
  raise 'leaf missing or repeated at the model' unless expected.all? { |tag| telemetry[:counts][tag] == 1 && telemetry[:finished][tag] == 1 }
  raise 'durable Worker identities are missing/duplicated' unless ledger.size == 19 && ledger.all? { |r| r['status'] == 'completed' && r['parent_session_id'] == parent } && ledger.map { |r| r['request_id'] }.uniq.size == 19 && ledger.map { |r| r['agent_id'] }.uniq.size == 19
  owners = ledger.group_by { |r| r.dig('workflow_owner', 'workflow_run_id') }.transform_values(&:size)
  raise "workers did not share the parent team: #{owners}" unless owners[nil] == 1 && complete.all? { |r| owners[r['id']] == 9 }
  messages = fixture_api(env, 'session.export', id: parent).dig('core', 'messages')
  batch = messages.find { |m| m.fetch('tool_calls', []).size == 3 }
  raise 'parent did not dispatch one Worker and two workflows in one model response' unless batch && batch['tool_calls'].map { |c| c['name'] }.sort == %w[spawn_agent workflow_control workflow_control]
  { check: 'same_turn_worker_two_workflows_shared_backpressure_19_unique_completions', passed: true, parent: parent, turn: turn,
    default_limits: { max_concurrent: 8, max_queued: 64 }, tested_limits: { max_concurrent: 3, max_queued: 2 }, queue_saturated: saturated, ownership_counts: owners.map { |id, count| { workflow: id, count: count } }, samples: samples, telemetry: telemetry }
ensure
  fixture.release(:load)
end

if $PROGRAM_NAME == __FILE__
  options = { output: File.join(ROOT, 'build/verification/workflow-runtime'), tui: true }
  OptionParser.new do |p|
    p.banner = 'Usage: ruby scripts/verify_workflow_runtime.rb [--serve] [--skip-tui] [--output DIR] (WILLDEEP_BIN selects an already-built binary)'
    p.on('--serve', 'Keep the isolated Web fixture alive after checks') { options[:serve] = true }
    p.on('--skip-tui', 'Report real PTY checks as skipped') { options[:tui] = false }
    p.on('--output DIR', 'JSON/Markdown and terminal evidence directory') { |dir| options[:output] = File.expand_path(dir) }
    p.on('-h', '--help') { puts p; exit }
  end.parse!
  fixture, results = WorkflowModelFixture.new, []
  output = options[:output]; FileUtils.mkdir_p(output)
  version, version_error, version_status = Open3.capture3(BIN, '--version')
  raise version_error unless version_status.success?
  report = { passed: false, version: version.strip, loopback_only: true, checks: results }
  Dir.mktmpdir('wd-workflow-', '/private/tmp') do |root|
    home, workspace = File.join(root, 'home'), File.join(root, 'workspace')
    FileUtils.mkdir_p([home, workspace], mode: 0o700)
    File.write(File.join(home, 'config.toml'), "version=1\ndefault_provider=\"fixture\"\n[agent]\nmax_turns=16\nsafety_judge=false\nauto_title=false\ninput_suggestions=false\nsmall_model_routing=false\nauto_dispatch_read_only=false\n[agent_team]\nmax_concurrent=3\nmax_queued=2\n[local_model]\nenabled=false\n[providers.fixture]\nprovider=\"openai-compatible\"\napi=\"chat-completions\"\napi_base=\"http://127.0.0.1:#{fixture.port}/v1\"\napi_key=\"fixture-only\"\nmodel=\"fixture-model\"\n", perm: 0o600)
    env = workflow_fixture_env(home)
    web_pid = nil
    begin
      fixture_api(env, 'workspace.register', root: workspace, name: '工作流隔离验收', access: 'full_access', provider_profile: 'fixture')
      parent = fixture_api(env, 'session.create', workspace: workspace, title: '工作流编排与智能体管理', profile: 'fixture').fetch('id')
      if options[:serve]
        port = Integer(ENV.fetch('WILLDEEP_SURFACE_PORT', '19378'))
        log = File.open(File.join(root, 'web.log'), 'w')
        web_pid = Process.spawn(env, BIN, '--web', '--listen', "127.0.0.1:#{port}", '-w', workspace, '-c', env.fetch('WILLDEEP_CONFIG'), '--language', 'zh-CN', out: log, err: log)
        preview = { root: root, home: home, config: env.fetch('WILLDEEP_CONFIG'), workspace: workspace, parent: parent, url: "http://127.0.0.1:#{port}" }
        File.write(File.join(output, 'fixture.json'), JSON.pretty_generate(preview) + "\n")
        puts JSON.generate(preview); $stdout.flush
      end
      turn = fixture_api(env, 'turn.submit', session_id: parent, turn_request_id: SecureRandom.uuid, prompt: 'CHAT_LIBRARY_E2E: create, dispatch, update and bind the fixture specialist.', origin_client: 'workflow-fixture')
      wait_for('chat library turn') { fixture_api(env, 'session.get', id: parent)['status'] == 'idle' }
      old, newer = fixture.observations.find { |e| e[:old] }, fixture.observations.find { |e| e[:new] }
      raise 'chat did not dispatch both specialist versions' unless old && newer
      raise "old snapshot changed: #{old}" unless old[:v1] && !old[:v2] && old[:model] == 'fixture-model'
      raise "new snapshot not applied: #{newer}" unless newer[:v2] && newer[:model] == 'fixture-model-v2'
      results << { check: 'chat_create_update_bind_same_turn_and_old_worker_snapshot', passed: true, turn: turn, old: old, new: newer }
      pipeline = workflow_spec('pipeline')
      fixture_api(env, 'workflow.validate', parent_session_id: parent, spec: pipeline)
      fixture_api(env, 'workflow.save', parent_session_id: parent, spec: pipeline)
      id = SecureRandom.uuid
      admission = fixture_api(env, 'workflow.run', parent_session_id: parent, definition_id: 'pipeline', input: {}, request_id: id).fetch('workflow')
      completed = wait_workflow(env, parent, id)
      raise "pipeline did not complete: #{completed}" unless completed['status'] == 'completed' && completed.dig('output', 'ok')
      replay = fixture_api(env, 'workflow.run', parent_session_id: parent, definition_id: 'pipeline', input: {}, request_id: id).fetch('workflow')
      raise 'replay changed run identity' unless replay['id'] == id
      results << { check: 'durable_api_admission_pipeline_and_idempotency', passed: true, admission: admission['status'], run: id, nodes: completed['nodes'].size }
      fixture_api(env, 'workflow.save', parent_session_id: parent, spec: workflow_spec('retry-pipeline', 'FAIL_FIRST_WORKFLOW_LEAF'))
      failed = fixture_api(env, 'workflow.run', parent_session_id: parent, definition_id: 'retry-pipeline', request_id: SecureRandom.uuid).fetch('workflow')
      failed = wait_workflow(env, parent, failed.fetch('id'))
      raise 'bad JSON did not fail the run' unless failed['status'] == 'failed'
      retried = fixture_api(env, 'workflow.retry', parent_session_id: parent, run_id: failed.fetch('id'), request_id: SecureRandom.uuid).fetch('workflow')
      retried = wait_workflow(env, parent, retried.fetch('id'))
      raise "retry did not preserve origin: #{retried.slice('id', 'source_run_id', 'status', 'error', 'nodes')}" unless retried['status'] == 'completed' && retried['source_run_id'] == failed['id']
      raise 'retry changed old history' unless fixture_api(env, 'workflow.get', parent_session_id: parent, run_id: failed.fetch('id')).dig('workflow', 'status') == 'failed'
      results << { check: 'json_rejection_retry_and_immutable_history', passed: true, source: failed['id'], run: retried['id'] }
      fixture_api(env, 'workflow.save', parent_session_id: parent, spec: workflow_spec('stop-pipeline', 'SLOW_WORKFLOW_LEAF'))
      stopped = fixture_api(env, 'workflow.run', parent_session_id: parent, definition_id: 'stop-pipeline', request_id: SecureRandom.uuid).fetch('workflow')
      wait_for('running leaf') { fixture_api(env, 'workflow.get', parent_session_id: parent, run_id: stopped['id']).fetch('workflow')['nodes'].any? { |n| n.dig('agent_run', 'status') == 'running' } }
      fixture_api(env, 'workflow.stop', parent_session_id: parent, run_id: stopped['id'], request_id: SecureRandom.uuid)
      stopped = wait_workflow(env, parent, stopped['id'])
      raise 'stop did not cancel graph' unless stopped['status'] == 'cancelled'
      results << { check: 'stop_cancels_real_worker_and_blocks_successor', passed: true, run: stopped['id'] }
      wait_for('parent aggregate acknowledgement') { fixture_api(env, 'workflow.list', parent_session_id: parent).fetch('runs').all? { |run| run['acknowledged'] } }
      results << { check: 'terminal_aggregates_acknowledged_after_parent_commit', passed: true }
      chat = fixture_api(env, 'session.create', workspace: workspace, title: 'Chat workflow fixture', profile: 'fixture').fetch('id')
      fixture_api(env, 'turn.submit', session_id: chat, turn_request_id: SecureRandom.uuid, prompt: 'CHAT_WORKFLOW_E2E', origin_client: 'workflow-fixture')
      wait_for('parent workflow final reply') { fixture.telemetry[:events].any? { |e| e[:event] == 'chat_final' } }
      wait_for('parent final persisted while phases continue') { fixture_api(env, 'session.export', id: chat).dig('core', 'messages').any? { |m| m['role'] == 'assistant' && m['content'].include?('CHAT_WORKFLOW_FINAL') } }
      admitted = fixture_api(env, 'workflow.list', parent_session_id: chat).fetch('runs').fetch(0)
      raise 'workflow phases finished before parent final was persisted' unless admitted['status'] == 'running' && admitted['nodes'].all? { |n| n['status'] != 'completed' }
      chat_run = wait_workflow(env, chat, admitted['id'])
      raise 'chat-started phases did not complete' unless chat_run['status'] == 'completed' && chat_run['nodes'].all? { |n| n['status'] == 'completed' }
      wait_for('chat final persisted') { fixture_api(env, 'session.get', id: chat)['status'] == 'idle' }
      results << verify_workflow_projection(env, workspace, chat, chat_run['id'], output)
      results << verify_item_pipeline(env, fixture, workspace)
      recovery = verify_recovery(env, fixture, root, workspace); results << recovery
      results << verify_shared_scheduler(env, fixture, workspace)
      metadata = { root: root, home: home, config: env.fetch('WILLDEEP_CONFIG'), workspace: workspace, parent: parent, tui_parent: recovery[:parent], run: recovery[:run], chat_parent: chat }
      File.write(File.join(output, 'fixture.json'), JSON.pretty_generate(metadata) + "\n")
      if options[:tui]
        # Keep PTY child handling outside the multithreaded HTTP model fixture.
        tui_output, tui_error, tui_status = Open3.capture3(RbConfig.ruby, '-EUTF-8',
          File.join(ROOT, 'scripts/verify_workflow_tui.rb'), '--fixture', File.join(output, 'fixture.json'),
          '--binary', BIN, '--output', output)
        raise "PTY verification failed: #{tui_error} #{tui_output}" unless tui_status.success?
        results << JSON.parse(tui_output, symbolize_names: true)
      else
        results << { check: 'real_pty_workflow_commands', passed: nil, status: 'skipped', reason: '--skip-tui' }
      end
      report.merge!(passed: true, coverage_complete: options[:tui], model_requests: fixture.requests.size)
      write_workflow_report(output, report)
      puts JSON.pretty_generate(report)
      if options[:serve]
        metadata[:url] = "http://127.0.0.1:#{port}"
        File.write(File.join(output, 'fixture.json'), JSON.pretty_generate(metadata) + "\n")
        puts JSON.generate(metadata); $stdout.flush
        trap('TERM') { exit }; trap('INT') { exit }
        sleep 1 while true
      end
    rescue StandardError => error
      report.merge!(passed: false, error: "#{error.class}: #{error.message}", model_requests: fixture.requests.size, telemetry: fixture.telemetry)
      write_workflow_report(output, report)
      warn "isolated fixture failure: #{error.class}: #{error.message}"
      raise
    ensure
      fixture.release(:load); fixture.release(:recovery)
      if web_pid
        Process.kill('TERM', web_pid) rescue Errno::ESRCH
        Process.wait(web_pid) rescue Errno::ECHILD
      end
      Open3.capture3(env, BIN, 'daemon', 'stop')
    end
  end
  fixture.close
end
