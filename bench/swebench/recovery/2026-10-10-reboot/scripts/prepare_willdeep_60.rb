require 'json'
require 'yaml'
require 'digest'
require 'fileutils'
require 'time'

SITE = '/Users/rocky/Sites/willdeep-react-static'
TASKS = '/private/tmp/willdeep-swebench-tasks/tasks'
ASSETS = '/private/tmp/willdeep-swebench-094rc3-assets'
suite = JSON.parse(File.read("#{SITE}/public/benchmarks/taskset.json"))
raise 'Frozen suite must contain exactly 60 unique tasks' unless suite['tasks'].size == 60 && suite['tasks'].map { |t| t['id'] }.uniq.size == 60
old = %w[2026-10-08-default-30 2026-10-09-glm5-30].flat_map { |dir| Dir.glob("/Users/rocky/Sites/willdeep-rs/bench/swebench/runs/#{dir}/**/*").select { |p| File.file?(p) } }
old += Dir.glob("#{SITE}/public/benchmarks/runs/{deepseek-flash-20261008,glm5-20261009}/**/*").select { |p| File.file?(p) }
hashes = old.to_h { |p| [p, Digest::SHA256.file(p).hexdigest] }
dataset = suite['tasks'].map do |task|
  dir = "#{TASKS}/#{task.fetch('id')}"
  item = YAML.safe_load(File.read("#{dir}/task.yaml"), permitted_classes: [Time, Date])
  raise 'Task base does not match frozen suite' unless item['base_commit'] == task['base_commit']
  item.merge('problem_statement' => File.read("#{dir}/problem_statement.md"))
end
%w[deepseek-flash glm5].each do |name|
  id = "cli-094rc3-#{name}-60-20261009-v1"
  root = "/private/tmp/willdeep-swebench-094rc3-#{name}-60-20261009-v1"
  raise "Run directory already exists: #{root}" if File.exist?(root)
  FileUtils.mkdir_p(root, mode: 0o700)
  source = name == 'glm5' ? '/private/tmp/willdeep-swebench-glm5-30-20261009' : '/private/tmp/willdeep-swebench-30-20261008'
  model = name == 'glm5' ? 'glm-5' : 'deepseek-v4-flash'
  config = File.read("#{source}/config.toml")
  config = config.gsub(/^model\s*=\s*"[^"]*"/, "model = \"#{model}\"")
  File.write("#{root}/config.toml", config, perm: 0o600)
  File.write("#{root}/dataset.json", JSON.pretty_generate(dataset), perm: 0o600)
  File.write("#{root}/taskset.json", JSON.pretty_generate(suite), perm: 0o600)
  File.write("#{root}/previous-report-sha256.json", JSON.pretty_generate(hashes), perm: 0o600)
  meta = JSON.parse(File.read("#{source}/manifest.json"))
  meta.merge!('run_id' => id, 'model' => model, 'binary_version' => '0.94.0-rc3', 'total' => 60,
    'instance_ids' => suite['tasks'].map { |t| t['id'] }, 'taskset' => suite['id'], 'taskset_sha256' => suite['sha256'],
    'selection' => suite['selection'], 'quotas' => suite['quotas'], 'created_at' => Time.now.iso8601,
    'binary_tag_commit' => 'e1b66d2a7c7a2c616efc0bced13ef1eb6545d702',
    'binary_asset_url' => 'https://github.com/niuwoai/willdeep-rs/releases/download/v0.94.0-rc3/willdeep-linux-amd64.tar.gz',
    'linux_binary_sha256' => nil, 'binary_path' => "#{ASSETS}/willdeep", 'runner_version' => '0.2.0',
    'pilot_note' => 'No pilot answers included. All 60 tasks are fresh executions with the released binary.',
    'paired_baseline_run_id' => nil, 'concurrency' => 1,
    'preservation_note' => 'Previous reports are immutable; new run directories, predictions, sessions and archives are separate.')
  File.write("#{root}/manifest.json", JSON.pretty_generate(meta), perm: 0o600)
  File.write("#{root}/progress.json", JSON.pretty_generate({stage: 'prepared', total: 60}), perm: 0o600)
  puts "prepared #{id}: 60 tasks; #{hashes.size} previous report files protected"
end

runner = File.read('/private/tmp/willdeep_swebench_glm_run.rb')
runner.sub!("ROOT = '/private/tmp/willdeep-swebench-glm5-30-20261009'", "ROOT = File.expand_path(ARGV.fetch(0))\nraise 'Unexpected run directory' unless ROOT.start_with?('/private/tmp/willdeep-swebench-094rc3-')")
runner.sub!("BINARY = '/private/tmp/willdeep-swebench-venv/willdeep'", "BINARY = '/private/tmp/willdeep-swebench-094rc3-assets/willdeep'")
runner.sub!("RUN = 'willdeep-092rc1-glm5-30-20261009-v1'", "RUN = MANIFEST.fetch('run_id')\nraise 'Release binary digest mismatch' unless File.file?(BINARY) && Digest::SHA256.file(BINARY).hexdigest == MANIFEST.fetch('linux_binary_sha256')\nraise 'Fixed task order mismatch' unless DATA.map { |t| t.fetch('instance_id') } == MANIFEST.fetch('instance_ids')\nraise 'Expected 60 tasks' unless DATA.size == 60")
runner.gsub!('total: 30', 'total: 60')
runner.gsub!('planned: 30', 'planned: 60')
runner.gsub!('resolved / 30.0', 'resolved / 60.0')
runner.gsub!('/30', '/60')
runner.gsub!('Verified 30 题', 'Verified 60 题')
runner.sub!("name = \"willdeep-glm5-sweb30-\#{index}\"", "name = \"\#{RUN}-\#{index}\"")
runner.sub!('name = \'willdeep-glm5-budget-smoke\'', 'name = "#{RUN}-budget-smoke"')
runner.sub!('--model glm-5 run', '--model MODEL_NAME run')
runner.sub!("code, timeout, elapsed, interrupted, reported_tokens, budget_stopped = run_agent(", "shell = shell.sub('MODEL_NAME', MANIFEST.fetch('model'))\n    File.write(\"\#{slot}/agent-started.json\", JSON.generate({started_at: Time.now.iso8601, model: MANIFEST.fetch('model')}), perm: 0o600)\n    code, timeout, elapsed, interrupted, reported_tokens, budget_stopped = run_agent(")
runner.sub!("  if File.file?(\"\#{slot}/status.json\")", "  if File.file?(\"\#{slot}/agent-started.json\") && !File.file?(\"\#{slot}/prediction.jsonl\")\n    abort 'Model execution already started without a saved prediction; manual reconciliation required, never rerun automatically.'\n  end\n  if File.file?(\"\#{slot}/status.json\")")
runner.sub!("    command(['docker', 'cp', \"\#{name}:/opt/willdeep-state\", \"\#{slot}/collected\"], \"\#{slot}/collect-state\")", "    collect_state(name, slot)")
runner.sub!("      command(['docker', 'cp', \"\#{name}:/opt/willdeep-state\", \"\#{slot}/collected\"], \"\#{slot}/collect-interrupted-state\", 60)", "      collect_state(name, slot)")
runner.sub!("  if %w[infrastructure_error evaluation_error interrupted].include?(row[:status])", "  if row[:status] == 'evaluation_error'\n    File.write(\"\#{slot}/deferred-grading.json\", JSON.generate({reason: 'Official grading unavailable; prediction preserved for controlled retry', created_at: Time.now.iso8601}))\n    next\n  end\n  if %w[infrastructure_error interrupted].include?(row[:status])")
runner.sub!("def usage_summary\n", "def collect_state(name, slot)\n  FileUtils.mkdir_p(\"\#{slot}/collected\", mode: 0o700)\n  %w[sessions tool-outputs usage workers].each do |part|\n    _, _, available = Open3.capture3('docker', 'exec', name, 'test', '-e', \"/opt/willdeep-state/\#{part}\")\n    next unless available.success?\n    checked(['docker', 'cp', \"\#{name}:/opt/willdeep-state/\#{part}\", \"\#{slot}/collected/\#{part}\"], \"\#{slot}/collect-\#{part}\", 180)\n  end\nend\n\ndef usage_summary\n")
runner.sub!('Dir.glob("#{ROOT}/instances/*/collected/usage/*.jsonl").flat_map', '(Dir.glob("#{ROOT}/instances/*/collected/usage/*.jsonl") + Dir.glob("#{ROOT}/instances/*/budget-usage.jsonl")).flat_map')
runner.sub!("  result = {model_calls: records.size", "  records.uniq! { |record| record.fetch('id') }\n  result = {kinds: records.group_by { |r| r['kind'] }.transform_values { |rs| {calls: rs.size, reported_total_tokens: rs.sum { |r| r['total_tokens'].to_i }} }, model_calls: records.size")
runner.sub!("summary\nDATA.each_with_index", "File.write(\"\#{ROOT}/jobs.json\", JSON.pretty_generate({pid: Process.pid, runner: '/private/tmp/willdeep_swebench_60_run.rb', started_at: Time.now.iso8601}))\nsummary\nDATA.each_with_index")
File.write('/private/tmp/willdeep_swebench_60_run.rb', runner, perm: 0o600)
