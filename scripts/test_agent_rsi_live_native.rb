#!/usr/bin/env ruby
# frozen_string_literal: true
# Real native Worker/tool/broker integration, DETERMINISTIC local fake upstream.
# Never reads a provider key, never sends inference, never claims model quality.
require 'optparse'
require 'open3'
require 'tmpdir'
require_relative 'lib/agent_rsi_live_host'

options = {}
OptionParser.new do |parser|
  %i[rust mac_root mac_frozen out].each { |key| parser.on('--' + key.to_s.tr('_', '-') + ' VALUE') { |value| options[key] = File.expand_path(value) } }
end.parse!
abort '--out and at least --rust or --mac-root are required' unless options[:out] && (options[:rust] || options[:mac_root])
abort 'output must be new' if File.exist?(options[:out])
FileUtils.mkdir_p(options[:out], mode: 0o700)
root = AgentRsiLive::Host::ROOT
suite = JSON.parse(File.read(File.join(root, 'bench/agent-rsi/reviewer-original-24-v1/suite.json')))
task = suite.fetch('tasks').first
golden = JSON.parse(File.read(File.join(root, 'docs/examples/agent-rsi-golden.json'))).dig('values', 'candidate')
AgentRsiLive.write(File.join(options[:out], 'package.json'), golden.fetch('baseline_package'))
manifest = {'proposal_sha256' => AgentRsiLive::PROPOSAL, 'maximum_bill_microusd_per_million_tokens' => 694_050,
            'expected_routed_model' => 'offline-fixture-route'}
ledger = AgentRsiLive::Ledger.new(options[:out], manifest)
calls = 0
transport = lambda do |request|
  calls += 1
  body = JSON.parse(request.body)
  last = body.fetch('messages').last
  message, finish = if last['role'] == 'tool'
    [{'role' => 'assistant', 'content' => task.fetch('expected_output')}, 'stop']
  else
    [{'role' => 'assistant', 'content' => nil, 'tool_calls' => [
      {'id' => 'fixture-read', 'type' => 'function', 'function' => {'name' => 'read_file', 'arguments' => JSON.generate(path: 'fixture.txt')}}
    ]}, 'tool_calls']
  end
  response = Struct.new(:code, :body, :headers) { def [](key) = headers[key] }
  response.new('200', JSON.generate({'id' => "offline-fixture-response-#{calls}", 'model' => 'opaque-offline-model',
    'choices' => [{'index' => 0, 'message' => message, 'finish_reason' => finish}],
    'usage' => {'prompt_tokens' => 128, 'completion_tokens' => 32, 'total_tokens' => 160}}),
    {'x-request-id' => "offline-fixture-request-#{calls}", 'x-routed-model' => 'offline-fixture-route'})
end
broker = AgentRsiLive::Broker.new(ledger, upstream: 'https://invalid.example/v1/chat/completions', api_key: 'FAKE-NOT-A-PROVIDER-KEY',
                                 directory: options[:out], transport: transport)
results = []
begin
  targets = []
  targets << ['rust-native-worker', [options[:rust], 'agents', 'rsi', 'native-trial'], root] if options[:rust]
  if options[:mac_root]
    abort '--mac-frozen required for Mac' unless options[:mac_frozen]
    targets << ['macos-native-worker', ['ruby', File.join(options[:mac_root], 'scripts/agent_rsi_approved_trial.rb'), '--execute', '--frozen-runner', options[:mac_frozen]], options[:mac_root]]
  end
  targets.each do |platform, command, cwd|
    trial = {'trial_id' => SecureRandom.uuid, 'platform' => platform, 'task_id' => task['task_id'], 'split' => task['split'],
             'arm' => 'baseline', 'repetition' => 1, 'attempt' => 1, 'resume_of' => nil, 'agent_revision' => AgentRsiLive::REVISIONS['baseline']}
    ledger.begin_trial(trial)
    output = File.join(options[:out], SecureRandom.uuid)
    control_path = File.join(options[:out], 'control.json')
    control = {'schema' => 'willdeep.agent-rsi.native-trial-control.v1', 'proposal_sha256' => AgentRsiLive::PROPOSAL,
               'experiment_id' => 'free-local-broker-integration', 'suite' => File.join(root, 'bench/agent-rsi/reviewer-original-24-v1/suite.json'),
               'suite_sha256' => AgentRsiLive::SUITE, 'agent_package' => File.join(options[:out], 'package.json'),
               'broker_origin' => broker.origin, 'capability' => broker.activate(trial['trial_id']), 'output_directory' => output}
    control.merge!(trial.reject { |key, _| %w[platform split].include?(key) })
    AgentRsiLive.write(control_path, control)
    stdout, stderr, status = Open3.capture3(*command, '--control', control_path, chdir: cwd)
    observation_path = File.join(output, 'observation.json')
    observation = File.exist?(observation_path) ? JSON.parse(File.read(observation_path)) : {}
    verification = AgentRsiLive::Grader.grade(observation['raw_model_content'].to_s, JSON.parse(task['expected_output']),
      read_observed: observation['read_observed'], fixture_unchanged: observation['fixture_unchanged'])
    success = status.success? && observation['loaded_revision'] == trial['agent_revision'] && verification['passed'] &&
              observation['status'] == 'unverified' && observation['did_truncate'] == false && ledger.state['paused'].nil?
    results << {'platform' => platform, 'passed' => success, 'native_exit_status' => status.exitstatus,
                'status' => observation['status'], 'fixture_read' => observation['read_observed'],
                'observation' => observation_path, 'quality_claim' => false,
                'stdout_sha256' => Digest::SHA256.hexdigest(stdout), 'stderr_sha256' => Digest::SHA256.hexdigest(stderr)}
    ledger.finish_trial(success ? 'fixture_acceptance_passed' : 'fixture_acceptance_failed')
    broker.deactivate
    File.delete(control_path)
  end
ensure
  broker.close
  ledger.close
end
report = {'schema' => 'willdeep.agent-rsi.free-native-broker-test.v1', 'passed' => results.all? { |row| row['passed'] },
          'paid_model_calls' => 0, 'provider_keys_read' => false, 'simulated_requests' => calls, 'results' => results,
          'costs_and_usage_are_synthetic_fixtures' => true, 'model_quality_claimed' => false}
AgentRsiLive.write(File.join(options[:out], 'native-broker-free.json'), report)
AgentRsiLive.write(File.join(options[:out], 'native-broker-free.md'), "# 原生 Worker 与预算代理免费联调\n\n通过：#{report['passed']}。模拟请求 #{calls}；真实模型调用0、未读取推理凭据。\n\n这是固定答案的工程验收，不代表模型质量。\n")
puts JSON.generate(report)
exit(report['passed'] ? 0 : 1)
