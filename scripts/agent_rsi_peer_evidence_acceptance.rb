# frozen_string_literal: true
require 'json'
require 'net/http'
require 'uri'
require 'fileutils'
require 'open3'
require 'digest'
require_relative 'lib/agent_rsi_contract'

repository = File.expand_path('..', __dir__)

receipt = JSON.parse(File.read(ENV.fetch('RSI_TEST_RECEIPT')))
origin = URI(receipt.fetch('origin'))
abort('isolated service required') unless receipt['mock_only'] == true && origin.scheme == 'http' && %w[127.0.0.1 localhost].include?(origin.host)
id = ENV.fetch('RSI_PEER_EXPERIMENT')
abort('invalid experiment ID') unless id.match?(/\A[a-zA-Z0-9._:\/-]{1,128}\z/)
out = ENV.fetch('RSI_ACCEPTANCE_OUTPUT')
FileUtils.mkdir_p(out)
read = lambda do |suffix|
  uri = URI(origin.to_s + '/api/v1/customer/agent-rsi/' + suffix)
  request = Net::HTTP::Get.new(uri)
  request['Authorization'] = 'Bearer ' + receipt.fetch('tokens').fetch('a')
  response = Net::HTTP.start(uri.host, uri.port, nil, nil, nil, nil, open_timeout: 5, read_timeout: 15) { |http| http.request(request) }
  raise "isolated HTTP #{response.code}" unless response.code == '200'
  JSON.parse(response.body).fetch('data')
end
spec_record = read.call('experiments/' + URI.encode_www_form_component(id))
spec = spec_record.fetch('document')
candidate = read.call('candidates/' + URI.encode_www_form_component(spec.fetch('candidate_id')))
records = read.call('experiments/' + URI.encode_www_form_component(id) + '/evidence').fetch('items')
raise 'peer evidence missing' unless records.size == 6
([spec_record, candidate] + records).each { |record| raise 'receipt digest mismatch' unless ContractGolden.hash(record.fetch('document')) == record.fetch('sha256') }
raise 'server trust escalation' unless records.all? { |r| r.dig('document', 'trust') == 'client_reported' }
paths = [candidate, spec_record, *records].each_with_index.map do |record, index|
  path = File.join(out, "peer-#{index}.json")
  File.write(path, JSON.pretty_generate(record.fetch('document')) + "\n")
  path
end
dependency = File.join(repository, 'target/debug/deps')
library = Dir.glob(File.join(dependency, 'libwilldeep_core-*.rlib')).max_by { |p| File.mtime(p) }
validator = File.join(out, 'rust-cross-validator')
stdout, stderr, status = Open3.capture3('rustc', '--edition=2024', '-L', 'dependency=' + dependency, '--extern', 'willdeep_core=' + library,
  File.join(__dir__, 'lib/agent_rsi_peer_validate.rs'), '-o', validator)
raise 'Rust validator build failed: ' + stderr unless status.success?
stdout, stderr, status = Open3.capture3(validator, *paths)
raise 'Rust cross-binding validation failed: ' + stderr unless status.success?
summary = {passed: true, scope: 'Mac native Worker -> authenticated some.im local service -> actual Rust core contract and binding validation',
  experiment_id: id, evidence_count: records.size, local_rust_library_sha256: Digest::SHA256.file(library).hexdigest,
  native_arms: records.map { |r| r.dig('document', 'arm') }.uniq.sort, paid_model_calls: 0, detail: stdout.strip}
File.write(File.join(out, 'mac-to-rust.json'), JSON.pretty_generate(summary) + "\n")
File.write(File.join(out, 'mac-to-rust.md'), "# Mac 到 Rust 证据互通\n\n原生 Mac 的 6 条双臂证据经真实本地客户鉴权 API 读取，服务回执 canonical SHA、client_reported 信任边界及实际 Rust core 候选/Spec/证据绑定校验均通过。\n\n实验：#{id}；付费请求：0。\n")
puts JSON.generate(summary)
