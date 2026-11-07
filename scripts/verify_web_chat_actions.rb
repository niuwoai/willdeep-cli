#!/usr/bin/env ruby
# frozen_string_literal: true
# API assertions against preview_web_chat_actions.rb only; no production config.
require 'json'
require 'net/http'
require 'uri'
require 'fileutils'
require 'securerandom'

fixture_path = File.expand_path(ARGV.fetch(0))
proposal_id = ARGV[1]
fixture = JSON.parse(File.read(fixture_path))
raise 'not an isolated Web fixture' unless fixture.fetch('root').start_with?('/private/tmp/willdeep-web-chat-actions-')
raise 'unexpected fixture port' unless fixture.fetch('port') == 19_862
base = "http://#{[127, 0, 0, 1].join('.')}:#{fixture.fetch('port')}"
checks = []
request = lambda do |path, body = nil|
  uri = URI(base + path)
  client = Net::HTTP.new(uri.host, uri.port, nil)
  client.read_timeout = 90
  message = body ? Net::HTTP::Post.new(uri) : Net::HTTP::Get.new(uri)
  message['Origin'] = base
  if body
    message['Content-Type'] = 'application/json'
    message.body = JSON.generate(body)
  end
  response = client.request(message)
  raise "fixture HTTP #{response.code}: #{path}" unless response.is_a?(Net::HTTPSuccess)
  response
end
json = lambda { |path, body = nil| JSON.parse(request.call(path, body).body) }
check = lambda do |name, &assertion|
  passed = !!assertion.call
  checks << { check: name, passed: passed }
  raise "verification failed: #{name}" unless passed
end
report = { version: '0.147.0-rc5', checks: checks, passed: false }
begin
  health = json.call('/health')
  check.call('fixture_version') { health['version'] == report[:version] }
  failed = fixture.fetch('failed')
  detail = json.call("/api/sessions/#{failed}")
  failure = detail.fetch('failure')
  check.call('historical_failure_contains_specific_model_cause') {
    failure.fetch('failure_detail').include?('preview-missing') && failure.fetch('failure_detail').include?('unavailable')
  }
  check.call('failure_has_authorized_session_task_and_turn') {
    failure['session_id'] == failed && failure['task_id'].is_a?(String) && failure['turn_id'].is_a?(String)
  }
  serialized = JSON.generate(failure)
  check.call('failure_redacts_credentials_and_private_paths') {
    !serialized.include?('preview-redaction-sentinel') && !serialized.include?('/private/tmp/') && !serialized.include?('private-workspace')
  }
  events = request.call("/api/sessions/#{failed}/stream?after=0").body.lines.filter_map do |line|
    JSON.parse(line.delete_prefix('data:').strip) if line.start_with?('data:')
  end
  terminal = events.reverse.find { |event| event['type'] == 'error' }
  check.call('terminal_reconnect_restores_the_same_failure') {
    terminal && terminal['failure_detail'] == failure['failure_detail'] && terminal['task_id'] == failure['task_id']
  }
  activity = json.call("/api/runtime/activity?workspace=#{URI.encode_www_form_component(fixture.fetch('workspace'))}")
  attention = activity.fetch('attention_sessions')
  check.call('attention_count_and_session_list_are_consistent') {
    activity['attention_count'] == attention.length && attention.map { |entry| entry['session_id'] }.uniq.length == attention.length
  }
  check.call('attention_lists_failure_and_pending_approval') {
    ids = attention.map { |entry| entry['session_id'] }
    ids.include?(failed) && ids.include?(fixture.fetch('approval'))
  }
  check.call('ordinary_completed_session_is_not_attention') {
    attention.none? { |entry| entry['session_id'] == fixture.fetch('main') }
  }
  if proposal_id
    path = "/api/sessions/#{fixture.fetch('main')}/children"
    before = json.call(path).fetch('tasks')
    confirmed = json.call("#{path}/proposals/#{proposal_id}/start", { request_id: SecureRandom.uuid })
    after = json.call(path).fetch('tasks')
    check.call('repeat_confirmation_reuses_browser_started_assignment') {
      before.any? { |task| task['assignment_id'] == confirmed['assignment_id'] } && before.length == after.length
    }
    proposals = json.call("#{path}/proposals").fetch('proposals')
    check.call('started_proposal_no_longer_offers_start') { proposals.none? { |proposal| proposal['id'] == proposal_id } }
  end
  report[:passed] = true
rescue StandardError => error
  report[:error] = "#{error.class}: #{error.message}"
ensure
  output = File.dirname(fixture_path)
  FileUtils.mkdir_p(output)
  File.write(File.join(output, 'api-report.json'), JSON.pretty_generate(report) + "\n")
  lines = checks.map { |entry| "- #{entry[:check]}：#{entry[:passed] ? '通过' : '失败'}" }
  File.write(File.join(output, 'api-report.md'), "# Web 聊天交互 API 验收\n\n版本：#{report[:version]}\n\n真实 Runtime、私有 HOME、本地确定性 Provider。\n\n#{lines.join("\n")}\n\n结果：#{report[:passed] ? '通过' : '失败'}。#{report[:error]}\n")
end
puts JSON.generate(report)
exit(report[:passed] ? 0 : 1)
