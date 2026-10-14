# frozen_string_literal: true
require 'json'
require 'net/http'
require 'securerandom'
require 'fileutils'

fixture = JSON.parse(File.read(ARGV.fetch(0)))
raise 'an isolated fixture is required' unless fixture.fetch('root').start_with?('/private/tmp/wdm-')
request = lambda do |method, path, body = nil|
  uri = URI(fixture.fetch('web_url') + path)
  message = Net::HTTP.const_get(method.capitalize).new(uri)
  message['Content-Type'] = 'application/json'
  message.body = JSON.generate(body) if body
  response = Net::HTTP.start(uri.host, uri.port, read_timeout: 10) { |http| http.request(message) }
  raise "#{method} #{path}: #{response.code}" unless response.is_a?(Net::HTTPSuccess)
  JSON.parse(response.body)
end
if ARGV[1] == '--seed'
  uri = URI(fixture.fetch('web_url') + '/api/chat/stream')
  message = Net::HTTP::Post.new(uri)
  message['Content-Type'] = 'application/json'
  message.body = JSON.generate(prompt: 'Inspect fixture.txt and report what is verified.', workspace: fixture.fetch('workspace'), language: 'en', attachments: [])
  response = Net::HTTP.start(uri.host, uri.port, read_timeout: 20) { |http| http.request(message) }
  raise 'initial fixture chat failed' unless response.is_a?(Net::HTTPSuccess)
  source = request.call('get', '/api/sessions').first.fetch('id')
  request.call('post', '/api/schedules', request_id: SecureRandom.uuid, task: {
    name: 'Review fixture results', prompt: 'Review the previous fixture result and report actual evidence and remaining unknowns.',
    workspace: fixture.fetch('workspace'), origin_session_id: source, continue_in_session: true,
    schedule: { kind: 'onceAt', at: Time.now.to_i + 2 }, timezone: 'Asia/Shanghai'
  })
end
tasks = request.call('get', '/api/schedules').fetch('tasks')
view = tasks.find { |item| item.dig('task', 'continue_in_session') && item.dig('task', 'origin_session_id') }
raise 'create a conversation task through the isolated Web first' unless view
task = view.fetch('task')
id = task.fetch('id')
source = task.fetch('origin_session_id')
before = request.call('get', '/api/sessions')
started = ARGV[1] == '--seed' ? nil : request.call('post', "/api/schedules/#{id}/run", request_id: SecureRandom.uuid)
deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + 45
loop do
  started ||= request.call('get', "/api/schedules/#{id}")['last_run']
  unless started
    raise 'timer did not trigger' if Process.clock_gettime(Process::CLOCK_MONOTONIC) >= deadline
    sleep 0.2
    next
  end
  record = request.call('get', "/api/schedules/#{id}/history/#{started.fetch('id')}")
  if %w[completed partial failed cancelled skipped unknown].include?(record.fetch('status'))
    raise "unexpected outcome: #{record['status']}" unless record['status'] == 'completed'
    raise 'run escaped its source conversation' unless record.fetch('session_id') == source
    raise 'history lost the task configuration' unless record.dig('snapshot', 'origin_session_id') == source
    raise 'a new run session was created' unless request.call('get', '/api/sessions').map { |session| session.fetch('id') }.sort == before.map { |session| session.fetch('id') }.sort
    calls = JSON.parse(File.read(File.join(fixture.fetch('root'), 'provider-requests.json')))
    last = calls.reverse.find { |call| call.fetch('path').include?('chat/completions') }
    messages = last.fetch('messages')
    raise 'prior context not sent to provider' unless messages.any? { |message| message['role'] == 'user' && message['content'].to_s.include?('Inspect fixture.txt') }
    raise 'scheduled instruction not sent to provider' unless messages.any? { |message| message['role'] == 'user' && message['content'].to_s.include?(task.fetch('prompt')) }
    raise 'current-run usage includes an older conversation turn' unless record.dig('usage', 'input_tokens') == 20 && record.dig('usage', 'output_tokens') == 3
    revision = request.call('get', "/api/schedules/#{id}").dig('task', 'revision')
    request.call('post', "/api/schedules/#{id}/action", action: 'pause', expected_revision: revision, request_id: SecureRandom.uuid)
    paused = request.call('get', "/api/schedules/#{id}")
    raise 'pause lost history or remained enabled' if paused.dig('task', 'enabled') || paused['last_run'].nil?
    report = {
      status: 'passed', version: request.call('get', '/health').fetch('version'),
      checks: %w[created_from_chat source_session_reused prior_context_sent task_instruction_sent history_retained pause_stops_future_triggers],
      trigger: record['trigger'],
      run_status: record['status'], verification: record['verification'] || 'unverified',
      source_session: source, run_session: record['session_id'], run_id: record['id'],
      model: record.dig('snapshot', 'model'), session_count: before.length,
      usage: record['usage']
    }
    directory = File.expand_path('../docs/verification/0.100.0-rc1', __dir__)
    FileUtils.mkdir_p(directory)
    File.write(File.join(directory, 'conversation-schedules.json'), JSON.pretty_generate(report) + "\n")
    File.write(File.join(directory, 'conversation-schedules.md'), "# 会话定时任务 Runtime 验收\n\n状态：通过。版本：#{report[:version]}。隔离假模型环境，无真实凭据。\n\n- Web 聊天内创建任务，绑定原会话。\n- 运行会话 ID 保持不变，未新增会话。\n- Provider 收到原聊天上下文和新定时指令。\n- 运行历史记录执行完成，验证状态仍为未验证。\n- 暂停停止后续计划并保留运行历史。\n\n本次证明续跑机制与历史链路；不证明真实模型质量提升。\n")
    puts JSON.generate(report)
    break
  end
  raise 'scheduled continuation did not finish' if Process.clock_gettime(Process::CLOCK_MONOTONIC) >= deadline
  sleep 0.2
end
