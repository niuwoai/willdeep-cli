#!/usr/bin/env ruby
# frozen_string_literal: true
# Composition regression: a writable P2 child must not gain machine configuration
# tools when its user takes control or detaches it from the active parent tree.
project = File.expand_path(ENV.fetch('WILLDEEP_SOURCE_ROOT', File.join(__dir__, '../..')))
require File.join(project, 'scripts/verify_child_sessions_p2')

class ConfigurationBoundaryFixture < ChildSessionModelFixture
  MARKERS = %w[INTEGRATED_ROOT_PROBE INTEGRATED_DELEGATED_PROBE INTEGRATED_OWNED_PROBE INTEGRATED_DETACHED_PROBE].freeze

  def initialize
    @configuration_probes = []
    super
  end

  def probes
    @mutex.synchronize { @configuration_probes.map(&:dup) }
  end

  def completion(request)
    text = JSON.generate(request.fetch('messages', []))
    marker = MARKERS.reverse.find { |value| text.include?(value) }
    return super unless marker
    names = request.fetch('tools', []).filter_map { |tool| tool.dig('function', 'name') }
    @mutex.synchronize { @configuration_probes << { marker: marker, tools: names } }
    { id: "boundary-#{SecureRandom.hex(8)}", object: 'chat.completion', model: request['model'],
      choices: [{ index: 0, message: { role: 'assistant', content: 'INTEGRATION_BOUNDARY_COMPLETE' }, finish_reason: 'stop' }],
      usage: { prompt_tokens: 100, completion_tokens: 10, total_tokens: 110 } }
  end
end

output = File.expand_path(ARGV.fetch(0))
FileUtils.mkdir_p(output)
fixture = ConfigurationBoundaryFixture.new
root = Dir.mktmpdir('wd-child-config-boundary-', '/private/tmp')
home, workspace = %w[home workspace].map { |name| File.join(root, name) }
FileUtils.mkdir_p([home, workspace], mode: 0o700)
checks = []
report = { passed: false, checks: checks, errors: [], root: root, loopback_only: true }
env = nil
cleanup_ok = false
begin
  config = [
    'version = 1', 'default_provider = "fixture"', '[agent]', 'approval = "full-access"',
    'max_turns = 8', 'safety_judge = false', 'auto_title = false', 'input_suggestions = false',
    'auto_dispatch_read_only = false', 'small_model_routing = false', '[local_model]', 'enabled = false',
    '[providers.fixture]', 'provider = "openai-compatible"', 'api = "chat-completions"',
    "api_base = \"http://127.0.0.1:#{fixture.port}/v1\"", 'api_key = "loopback-only"',
    "model = #{JSON.generate(fixture.fixture_list('GET /v1/models').fetch(:data).first.fetch(:id))}", ''
  ].join("\n")
  path = File.join(home, 'config.toml')
  File.write(path, config, perm: 0o600)
  before = Digest::SHA256.file(path).hexdigest
  File.write(File.join(workspace, 'evidence.txt'), "INTEGRATION_EVIDENCE\n")
  child_git(workspace, 'init', '--quiet')
  child_git(workspace, 'add', 'evidence.txt')
  child_git(workspace, 'commit', '--quiet', '-m', 'Synthetic configuration boundary baseline')
  env = workflow_fixture_env(home).merge('WILLDEEP_WORKSPACE_CHECKPOINTS' => '0')
  version, error, status = Open3.capture3(BIN, '--version')
  raise error unless status.success?
  report[:version] = version.strip
  report[:binary] = { path: File.realpath(BIN), sha256: Digest::SHA256.file(BIN).hexdigest }
  child_api(env, 'workspace.register', root: workspace, name: 'Integration boundary', access: 'workspace_write', provider_profile: 'fixture')
  parent = child_api(env, 'session.create', workspace: workspace, title: 'Configuration boundary parent', profile: 'fixture').fetch('id')
  root_turn = child_api(env, 'turn.submit', session_id: parent, turn_request_id: SecureRandom.uuid,
    prompt: 'INTEGRATED_ROOT_PROBE', origin_client: 'integration-boundary')
  wait_for('root configuration probe completes') do
    turn = child_api(env, 'turn.get', id: root_turn.fetch('id'))
    raise "root probe failed: #{turn['error']}" if %w[failed cancelled interrupted].include?(turn['status'])
    turn['status'] == 'completed'
  end
  root_probe = fixture.probes.find { |item| item[:marker] == 'INTEGRATED_ROOT_PROBE' }
  raise 'root tools were not observed' unless root_probe
  raise 'ordinary parent lacks configuration tools' unless (%w[configuration_read configuration_write] - root_probe[:tools]).empty?
  checks << { name: 'ordinary parent advertises both built-in configuration tools', passed: true }

  task = p2_start(env, parent, 'Writable child boundary', workspace_mode: 'worktree', prompt: 'CHILD_P1_WRITE INTEGRATED_DELEGATED_PROBE')
  finished = child_terminal(env, parent, task.fetch('assignment_id'))
  raise "delegated child failed: #{finished['error']}" unless finished['status'] == 'completed'
  child = task.fetch('child_session_id')
  delegated_session = child_api(env, 'session.get', id: child)
  raise 'read-only policy would mask the writable delegation boundary' if delegated_session['delegation_read_only']
  delegated_probe = fixture.probes.find { |item| item[:marker] == 'INTEGRATED_DELEGATED_PROBE' }
  raise 'writable child request was not observed' unless delegated_probe
  raise 'write-capable tool catalog was not observed' unless delegated_probe[:tools].include?('create_file')
  raise 'delegated child exposes configuration tools' unless (delegated_probe[:tools] & %w[configuration_read configuration_write]).empty?
  checks << { name: 'writable delegated child has no configuration host tools', passed: true }

  link = p2_link(env, parent, child)
  worktree_path = link.fetch('worktree').fetch('path')
  raise 'real writable worktree was not prepared' unless File.directory?(worktree_path)
  %w[takeover detach].each do |operation|
    state = operation == 'takeover' ? 'user_owned' : 'detached'
    marker = operation == 'takeover' ? 'INTEGRATED_OWNED_PROBE' : 'INTEGRATED_DETACHED_PROBE'
    link = child_api(env, "session.child.#{operation}", parent_session_id: parent,
      child_session_id: child, expected_control_revision: link.fetch('control_revision'))
    raise "unexpected control state after #{operation}" unless link['control_state'] == state
    user_turn = child_api(env, 'turn.submit', session_id: child, turn_request_id: SecureRandom.uuid,
      prompt: marker, origin_client: 'integration-boundary')
    wait_for("#{state} direct user turn completes") do
      turn = child_api(env, 'turn.get', id: user_turn.fetch('id'))
      raise "#{state} user turn failed: #{turn['error']}" if %w[failed cancelled interrupted].include?(turn['status'])
      turn['status'] == 'completed'
    end
    probe = fixture.probes.reverse.find { |item| item[:marker] == marker }
    raise "#{state} request was not observed" unless probe
    raise "#{state} child gained configuration tools" unless (probe[:tools] & %w[configuration_read configuration_write]).empty?
    session = child_api(env, 'session.get', id: child)
    raise "#{state} lost parent provenance" unless session['parent_session_id'] == parent
    raise "#{state} unexpectedly changed writable cap" if session['delegation_read_only']
    raise "#{state} changed selected configuration" unless Digest::SHA256.file(path).hexdigest == before
    original = child_api(env, 'session.child.task.get', parent_session_id: parent, assignment_id: task.fetch('assignment_id'))
    raise "#{state} rewrote original assignment result" unless p2_result_payload(original) == p2_result_payload(finished)
    checks << { name: "#{state} direct chat keeps configuration restriction, provenance and old assignment result", passed: true }
  end
  report[:passed] = true
rescue StandardError => error
  report[:errors] << "#{error.class}: #{error.message}"
ensure
  begin
    child_stop_daemon(env) if env
    cleanup_ok = true
  rescue StandardError => error
    report[:errors] << "cleanup: #{error.class}: #{error.message}"
    report[:passed] = false
  ensure
    fixture.close
  end
  report[:cleanup_completed] = cleanup_ok
  report[:observed_probes] = fixture.probes.map { |item| { marker: item[:marker], configuration_tools: item[:tools] & %w[configuration_read configuration_write] } }
  FileUtils.remove_entry(root) if cleanup_ok && report[:passed]
  report[:fixture_preserved] = File.directory?(root)
  File.write(File.join(output, 'report.json'), JSON.pretty_generate(report) + "\n")
  rows = checks.map { |item| "- #{item[:name]}：通过" }
  text = "# P2 与聊天配置组合权限验收\n\n版本：#{report[:version]}。结果：#{report[:passed] ? '通过' : '失败'}。真实 Runtime、合成 Git 工作区、回环模型；使用可写子会话，避免只读策略掩盖委派权限错误。\n\n#{rows.join("\n")}\n\n#{report[:errors].join("\n")}\n"
  File.write(File.join(output, 'report.md'), text)
end
puts JSON.pretty_generate(report)
exit(report[:passed] ? 0 : 1)
