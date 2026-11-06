#!/usr/bin/env ruby
# Black-box checks for the local workspace approval API.
require 'fileutils'
require 'json'
require 'net/http'
require 'optparse'
require 'uri'

options = {
  base_url: 'http://127.0.0.1:19461',
  report_dir: File.join(File.expand_path('../..', __dir__), '.build/verification/0.146.0-rc1/workspace-approval')
}
OptionParser.new do |parser|
  parser.on('--base-url URL') { |value| options[:base_url] = value }
  parser.on('--report-dir DIR') { |value| options[:report_dir] = value }
end.parse!

base = URI(options.fetch(:base_url))
raise 'base URL must use loopback HTTP' unless base.scheme == 'http' && %w[127.0.0.1 localhost].include?(base.host)

def api_request(base, path, method: Net::HTTP::Get, body: nil, origin: nil)
  uri = base + path
  request = method.new(uri)
  request['Accept'] = 'application/json'
  if body
    request['Content-Type'] = 'application/json'
    request.body = JSON.generate(body)
  end
  request['Origin'] = origin if origin
  response = Net::HTTP.start(uri.host, uri.port, proxy: nil, open_timeout: 5, read_timeout: 10) do |http|
    http.request(request)
  end
  payload = JSON.parse(response.body) rescue nil
  [response.code.to_i, payload]
rescue StandardError => error
  [-1, { 'error' => error.class.name }]
end

checks = []
def record(checks, name, passed, status, detail)
  checks << { name: name, passed: passed, status: status, detail: detail }
end

list_status, workspaces = api_request(base, '/api/workspaces')
valid_list = list_status == 200 && workspaces.is_a?(Array)
record(checks, 'workspace list is available', valid_list, list_status, 'GET /api/workspaces returns a JSON array')

if valid_list && (workspace = workspaces.find { |item| item.is_a?(Hash) && item['path'].is_a?(String) })
  path = workspace.fetch('path')
  initial_access = workspace['access']
  initial_configured = workspace['approval_mode_configured']

  invalid_status, = api_request(
    base,
    '/api/workspaces/approval-mode',
    method: Net::HTTP::Post,
    body: { path: path, approval_mode: 'workspace-write', confirm: false }
  )
  record(checks, 'unknown approval mode is rejected', invalid_status.between?(400, 499), invalid_status, 'invalid mode does not reach a successful write')

  confirmation_status, = api_request(
    base,
    '/api/workspaces/approval-mode',
    method: Net::HTTP::Post,
    body: { path: path, approval_mode: 'full_access', confirm: false }
  )
  record(checks, 'full access requires explicit confirmation', confirmation_status.between?(400, 499), confirmation_status, 'unconfirmed full access is rejected')

  read_only_status, = api_request(
    base,
    '/api/workspaces/approval-mode',
    method: Net::HTTP::Post,
    body: { path: path, approval_mode: 'read_only', confirm: false }
  )
  record(checks, 'read-only remains a workspace policy', read_only_status.between?(400, 499), read_only_status, 'read-only cannot be selected as an approval mode')

  after_failure_status, after_failures = api_request(base, '/api/workspaces')
  unchanged = after_failure_status == 200 && after_failures.is_a?(Array) &&
    (current = after_failures.find { |item| item['path'] == path }) &&
    current['access'] == initial_access && current['approval_mode_configured'] == initial_configured
  record(checks, 'rejected writes preserve workspace state', !!unchanged, after_failure_status, 'access and onboarding marker remain unchanged')

  valid_status, saved = api_request(
    base,
    '/api/workspaces/approval-mode',
    method: Net::HTTP::Post,
    body: { path: path, approval_mode: 'strict', confirm: false }
  )
  saved_ok = valid_status == 200 && saved.is_a?(Hash) && saved['access'] == 'strict' && saved['approval_mode_configured'] == true
  record(checks, 'valid strict selection is saved', saved_ok, valid_status, 'saved workspace reports strict and configured')

  persisted_status, persisted_workspaces = api_request(base, '/api/workspaces')
  persisted = persisted_workspaces.is_a?(Array) && persisted_workspaces.find { |item| item['path'] == path }
  persisted_ok = persisted_status == 200 && persisted && persisted['access'] == 'strict' && persisted['approval_mode_configured'] == true
  record(checks, 'strict selection persists in workspace list', !!persisted_ok, persisted_status, 'a subsequent read returns the saved mode and marker')
else
  record(checks, 'approval checks have a workspace target', false, list_status, 'no registered workspace was returned')
end

report = {
  suite: 'workspace approval API',
  base_url: "#{base.scheme}://#{base.host}:#{base.port}",
  passed: checks.all? { |check| check[:passed] },
  checks: checks
}
report_dir = File.expand_path(options.fetch(:report_dir))
FileUtils.mkdir_p(report_dir)
json_path = File.join(report_dir, 'workspace-approval-api.json')
markdown_path = File.join(report_dir, 'workspace-approval-api.md')
File.write(json_path, JSON.pretty_generate(report) + "\n")
markdown = [
  '# Workspace approval API checks',
  '',
  "Result: **#{report[:passed] ? 'PASS' : 'FAIL'}**",
  '',
  '| Check | Result | HTTP | Details |',
  '| --- | --- | ---: | --- |'
]
checks.each do |check|
  detail = check[:detail].gsub('|', '\\|')
  markdown << "| #{check[:name]} | #{check[:passed] ? 'PASS' : 'FAIL'} | #{check[:status]} | #{detail} |"
end
File.write(markdown_path, markdown.join("\n") + "\n")
puts JSON.generate(report)
puts "JSON report: #{json_path}"
puts "Markdown report: #{markdown_path}"
exit(report[:passed] ? 0 : 1)
