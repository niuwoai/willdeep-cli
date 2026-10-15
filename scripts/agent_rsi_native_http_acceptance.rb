# frozen_string_literal: true
require 'json'
require 'net/http'
require 'uri'
require 'open3'
require 'fileutils'
require 'securerandom'
require 'digest'
require_relative 'lib/agent_rsi_contract'

repository = File.expand_path('..', __dir__)

binary = ENV.fetch('RSI_TEST_BINARY', File.join(repository, 'target/debug/willdeep'))
receipt_path = ENV.fetch('RSI_TEST_RECEIPT')
receipt = JSON.parse(File.read(receipt_path))
uri = URI(receipt.fetch('origin'))
abort('only isolated loopback service allowed') unless %w[localhost 127.0.0.1].include?(uri.host) && uri.scheme == 'http' && receipt['mock_only'] == true
output = ENV.fetch('RSI_ACCEPTANCE_OUTPUT', '/private/tmp/willdeep-rsi-acceptance')
FileUtils.mkdir_p(output)
run_id = 'rust24-' + SecureRandom.hex(5)
directory = File.join(output, run_id)
FileUtils.mkdir_p(directory, mode: 0o700)
home = File.join(directory, 'home'); FileUtils.mkdir_p(home, mode: 0o700)
checks = []
values = JSON.parse(File.read(File.join(repository, 'docs/examples/agent-rsi-golden.json'))).fetch('values')
candidate = values.fetch('candidate')
%w[baseline candidate].each do |arm|
  File.write(File.join(directory, "#{arm}.json"), JSON.pretty_generate(candidate.fetch("#{arm}_package")) + "\n", mode: 'w', perm: 0o600)
end
environment = { 'WILLDEEP_HOME' => home, 'WILLDEEP_CONFIG' => nil, 'WILLDEEP_API_KEY' => nil, 'WILLDEEP_MODEL' => nil,
                'OPENAI_API_KEY' => nil, 'ANTHROPIC_API_KEY' => nil, 'WILLDEEP_TELEMETRY_DISABLED' => '1' }
call = lambda do |*arguments|
  stdout, stderr, status = Open3.capture3(environment, binary, 'agents', 'rsi', *arguments)
  File.write(File.join(directory, "#{arguments[0]}-stdout.json"), stdout)
  File.write(File.join(directory, "#{arguments[0]}-stderr.txt"), stderr)
  raise "#{arguments[0]} failed: #{stderr.lines.last(4).join.strip}" unless status.success?
  JSON.parse(stdout)
end
http = lambda do |method, suffix, body = nil|
  target = URI(receipt.fetch('origin') + '/api/v1/customer/' + suffix)
  klass = method == 'POST' ? Net::HTTP::Post : Net::HTTP::Get
  request = klass.new(target)
  request['Authorization'] = 'Bearer ' + receipt.fetch('tokens').fetch('a')
  if body
    request['Content-Type'] = 'application/json'
    request['Idempotency-Key'] = run_id + '-' + SecureRandom.hex(5)
    request.body = JSON.generate(body)
  end
  response = Net::HTTP.start(target.host, target.port, nil, nil, nil, nil, open_timeout: 5, read_timeout: 30) { |client| client.request(request) }
  raise "isolated HTTP #{response.code}" unless response.code.to_i == 200
  JSON.parse(response.body).fetch('data')
end
check = lambda do |name, &block|
  block.call
  checks << { 'name' => name, 'status' => 'passed' }
rescue StandardError => error
  checks << { 'name' => name, 'status' => 'failed', 'detail' => error.message }
end
before_binary = Digest::SHA256.file(binary).hexdigest
check.call('authenticated identity from isolated service') do
  identity = http.call('GET', 'agent-identity')
  session = { 'origin' => receipt['origin'], 'token' => receipt['tokens']['a'], 'user_id' => identity.fetch('user_id'), 'tenant_id' => identity.fetch('tenant_id') }
  File.write(File.join(home, 'agent-session.json'), JSON.generate(session), mode: 'w', perm: 0o600)
end
prepared = nil
check.call('prepare 24-task experiment without installing a candidate') do
  prepared = call.call('prepare', '--baseline', File.join(directory, 'baseline.json'), '--candidate', File.join(directory, 'candidate.json'), '--id', run_id, '--suite', File.join(repository, 'bench/agent-rsi/reviewer-original-24-v1/suite.json'))
  raise 'candidate activated' unless prepared['activated'] == false
  raise 'normal installation state created' if Dir.glob(File.join(home, '**', 'installations.json')).any?
end
result = nil
check.call('48 direct Worker executions with actual file tool and host verification') do
  result = call.call('run', run_id, '--agent-revision', candidate.dig('candidate_package', 'sha256'))
  raise 'binary changed during execution; use a fresh experiment' unless before_binary == Digest::SHA256.file(binary).hexdigest
end
report = nil
check.call('report has 48 unique bound evidence rows without promotion') do
  report = call.call('report', run_id)
  rows = report['rows'] || report['evidence'] || []
  if rows.empty?
    evidence_paths = Dir.glob(File.join(home, '**', 'trials', '*', 'evidence.json'))
    rows = evidence_paths.map { |p| JSON.parse(File.read(p)) }
  end
  raise "expected 48 evidence rows, got #{rows.length}; report keys #{report.keys.join(',')}" unless rows.length == 48
  raise 'target/verifier evidence missing' unless rows.all? { |r| r['target_executed'] && r.dig('verifier', 'result') == 'passed' && r['worker_run_id'] && r['events_sha256'] }
  raise 'duplicate independent sample' unless rows.map { |r| [r['task_id'], r['arm'], r['repetition']] }.uniq.length == 48
  raise 'normal installation state created' if Dir.glob(File.join(home, '**', 'installations.json')).any?
end
check.call('explicit resume retains sample count without rerunning completed work') do
  count_before = Dir.glob(File.join(home, '**', 'trials', '*', 'evidence.json')).length
  call.call('run', run_id, '--agent-revision', candidate.dig('candidate_package', 'sha256'), '--resume')
  count_after = Dir.glob(File.join(home, '**', 'trials', '*', 'evidence.json')).length
  raise 'resume duplicated terminal evidence' unless count_before == count_after && count_after == 48
end
grant = values.fetch('grant').merge('grant_id' => run_id + '-grant')
check.call('separate summary grant created using real customer auth') { http.call('POST', 'agent-rsi/grants', grant) }
upload = nil
check.call('Rust CLI uploads 48 admitted summaries through shared API') do
  upload = call.call('upload', run_id, '--grant-id', grant['grant_id'])
  raise "wrong upload count #{upload['uploaded_evidence'].inspect}" unless upload['uploaded_evidence'] == 48
  raise 'upload activation flag invalid' unless upload['activated'] == false && upload['trust'] == 'client_reported'
end
check.call('server preserves all Rust rows as client-reported evidence') do
  data = http.call('GET', "agent-rsi/experiments/#{run_id}/evidence")
  items = data.fetch('items')
  raise 'stored evidence count not 48' unless items.length == 48
  raise 'trust or digest mismatch' unless items.all? { |r| r.dig('document', 'trust') == 'client_reported' && ContractGolden.hash(r['document']) == r['sha256'] }
end
check.call('second CLI upload is idempotent') do
  again = call.call('upload', run_id, '--grant-id', grant['grant_id'])
  raise 'idempotent count changed' unless again['uploaded_evidence'] == 48
  raise 'duplicate service rows' unless http.call('GET', "agent-rsi/experiments/#{run_id}/evidence").fetch('items').length == 48
end
summary = { 'schema' => 'willdeep.agent-rsi.acceptance.v1', 'scope' => 'Rust production Worker with deterministic mock + isolated customer HTTP API',
            'experiment_id' => run_id, 'home' => home, 'binary_sha256' => before_binary, 'paid_model_calls' => 0,
            'expected_trials' => 48, 'suite_sha256' => ContractGolden.hash(JSON.parse(File.read(File.join(repository, 'bench/agent-rsi/reviewer-original-24-v1/suite.json')))),
            'checks' => checks, 'passed' => checks.all? { |r| r['status'] == 'passed' } }
File.write(File.join(output, 'rust-native-http.json'), JSON.pretty_generate(summary) + "\n")
File.write(File.join(output, 'rust-native-http.md'), "# Rust 原生离线与 HTTP 验收\n\n24 题双臂，共 48 次 Worker 执行；真实模型请求 0 次。\n\n| 检查 | 结果 |\n|---|---|\n" + checks.map { |r| "| #{r['name']} | #{r['status']}#{r['detail'] ? ': ' + r['detail'] : ''} |" }.join("\n") + "\n")
puts JSON.generate({ 'passed' => summary['passed'], 'experiment_id' => run_id, 'home' => home, 'checks' => checks.length, 'failures' => checks.select { |r| r['status'] == 'failed' } })
exit(summary['passed'] ? 0 : 1)
