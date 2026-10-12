#!/usr/bin/env ruby
# Exercise only an explicitly supplied, isolated management_fixture.
require 'json'
require 'net/http'
require 'securerandom'
require 'fileutils'

fixture = JSON.parse(File.read(ARGV.fetch(0)))
raise 'not an isolated fixture' unless fixture.fetch('root').start_with?('/private/tmp/wdm-')
base = fixture.fetch('web_url')
results = []
request = lambda do |method, path, body = nil|
  uri = URI(base + path)
  message = Net::HTTP.const_get(method.capitalize).new(uri)
  message['Content-Type'] = 'application/json'
  message.body = JSON.generate(body) if body
  response = Net::HTTP.start(uri.host, uri.port, read_timeout: 15) { |http| http.request(message) }
  raise "#{method} #{path}: #{response.code}: #{response.body}" unless response.is_a?(Net::HTTPSuccess)
  JSON.parse(response.body)
end
create = lambda do |name, prompt, extra|
  task = { name: name, prompt: prompt, workspace: fixture.fetch('workspace'), schedule: { kind: 'interval', minutes: 1440 }, profile: 'fixture', approval_mode: 'workspace-write', max_turns: 1, timeout_seconds: 30 }.merge(extra)
  request.call('post', '/api/schedules', task: task, request_id: SecureRandom.uuid).fetch('task')
end
run = lambda do |task|
  request.call('post', "/api/schedules/#{task.fetch('id')}/run", request_id: SecureRandom.uuid)
end
wait_run = lambda do |task, started|
  deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + 45
  loop do
    record = request.call('get', "/api/schedules/#{task.fetch('id')}/history/#{started.fetch('id')}")
    break record if %w[completed partial failed cancelled skipped unknown].include?(record.fetch('status'))
    raise 'run did not terminate' if Process.clock_gettime(Process::CLOCK_MONOTONIC) >= deadline
    sleep 0.2
  end
end
parameters = { schema: 'willdeep.runtime-parameters.v1', max_turns: 9, token_budget: 1_000_000, goal_token_budget: nil, goal_wall_clock_minutes: 240, goal_max_continuations: 64, input_suggestions: false, small_model_routing: false, auto_dispatch_read_only: false, max_deep_calls_per_harness: 1 }
File.write(File.join(fixture.fetch('home'), 'runtime-parameters.json'), JSON.pretty_generate(parameters))
File.chmod(0600, File.join(fixture.fetch('home'), 'runtime-parameters.json'))

count = lambda do
  JSON.parse(File.read(File.join(fixture.fetch('root'), 'provider-requests.json'))).count { |entry| entry.fetch('path').include?('chat/completions') }
rescue Errno::ENOENT
  0
end
budget = create.call('预算验收：任务 1 轮覆盖全局 9 轮', 'budget-loop read fixture.txt repeatedly', {})
before = count.call
record = wait_run.call(budget, run.call(budget))
calls = count.call - before
raise "budget failed: #{record['status']}, calls=#{calls}" unless record['status'] == 'partial' && calls == 1
results << { check: 'task_turn_budget_overrides_global', status: 'passed', actual_status: record['status'], model_calls: calls, snapshot_max_turns: record.dig('snapshot', 'max_turns'), run_id: record['id'] }

token_task = create.call('Token 预算覆盖全局', 'token-budget-loop read fixture.txt repeatedly', max_turns: 9, token_budget: 1000)
before = count.call
record = wait_run.call(token_task, run.call(token_task))
calls = count.call - before
raise "token budget failed: #{record['status']}, calls=#{calls}" unless record['status'] == 'failed' && record.fetch('reason').include?('token budget') && calls == 1 && record.dig('usage', 'total_tokens') == 1103
results << { check: 'task_token_budget_overrides_global', status: 'passed', actual_status: record['status'], model_calls: calls, reported_tokens: record.dig('usage', 'total_tokens'), run_id: record['id'] }

timeout = create.call('超时验收：1 秒终止 20 秒响应', 'slow wall clock test', timeout_seconds: 1)
before = count.call
started_at = Process.clock_gettime(Process::CLOCK_MONOTONIC)
record = wait_run.call(timeout, run.call(timeout))
elapsed = Process.clock_gettime(Process::CLOCK_MONOTONIC) - started_at
calls = count.call - before
raise "timeout failed: #{record['status']}, #{elapsed}, #{record['reason']}" unless record['status'] == 'failed' && elapsed < 6 && calls == 1
sleep 2
raise 'timeout unexpectedly replayed' unless count.call - before == 1
results << { check: 'wall_clock_timeout_no_replay', status: 'passed', actual_status: record['status'], elapsed_seconds: elapsed.round(2), model_calls: calls, reason: record['reason'], run_id: record['id'] }

normal = create.call('正常完成与用量记录', 'normal completion', {})
record = wait_run.call(normal, run.call(normal))
raise "completion failed: #{record}" unless record['status'] == 'completed' && record.dig('usage', 'total_tokens') == 23 && record['result'].include?('Fixture result: completed')
results << { check: 'completion_result_and_usage', status: 'passed', usage: record['usage'], verification: record['verification'], run_id: record['id'] }

task = request.call('get', "/api/schedules/#{normal.fetch('id')}").fetch('task')
archived = request.call('post', "/api/schedules/#{task.fetch('id')}/action", action: 'archive', expected_revision: task.fetch('revision'), request_id: SecureRandom.uuid).fetch('task')
raise 'archive lost history' unless request.call('get', "/api/schedules/#{task.fetch('id')}/history").fetch('runs').any? { |entry| entry['id'] == record['id'] }
request.call('post', "/api/schedules/#{task.fetch('id')}/action", action: 'delete', expected_revision: archived.fetch('revision'), request_id: SecureRandom.uuid)
raise 'deleted history lost' unless request.call('get', '/api/schedules/deleted').any? { |entry| entry.dig('task', 'id') == task['id'] }
results << { check: 'archive_delete_retains_history', status: 'passed' }

identity = SecureRandom.uuid
chat_body = { prompt: 'normal idempotent receipt test', request_id: identity, workspace: fixture.fetch('workspace'), language: 'en', attachments: [] }
chat = lambda do
  uri = URI(base + '/api/chat/stream')
  message = Net::HTTP::Post.new(uri)
  message['Content-Type'] = 'application/json'
  message.body = JSON.generate(chat_body)
  response = Net::HTTP.start(uri.host, uri.port, read_timeout: 5) { |http| http.request(message) }
  raise "chat: #{response.code}" unless response.is_a?(Net::HTTPSuccess)
  response.body.lines.filter_map { |line| JSON.parse(line.delete_prefix('data:')) if line.start_with?('data:') }
end
before = count.call
first = chat.call
again = chat.call
raise 'completed receipt retry did not complete' unless again.any? { |event| event['type'] == 'completed' }
first_turn = first.find { |event| event['type'] == 'submitted' }.fetch('turn_id')
again_turn = again.find { |event| event['type'] == 'submitted' }.fetch('turn_id')
raise 'receipt retry created duplicate execution' unless first_turn == again_turn && count.call - before == 1
results << { check: 'completed_chat_receipt_retry_is_idempotent', status: 'passed', model_calls: 1, turn_id: first_turn }

report = { version: '0.98.0-rc1', fixture: fixture['root'], tests: results }
output = ARGV[1] || File.join(fixture.fetch('root'), 'verification')
FileUtils.mkdir_p(output)
File.write(File.join(output, 'runtime-management.json'), JSON.pretty_generate(report))
File.write(File.join(output, 'runtime-management.md'), "# Runtime 管理实际运行验收\n\n版本：0.98.0-rc1。隔离 Provider，未调用真实付费模型。\n\n" + results.map { |entry| "- #{entry[:check]}：#{entry[:status]}；#{JSON.generate(entry)}" }.join("\n") + "\n")
puts JSON.pretty_generate(report)
