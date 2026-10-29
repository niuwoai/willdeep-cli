require 'json'
require 'yaml'
require 'date'
require 'digest'
require 'fileutils'
require 'time'

base = '/Users/rocky/Sites/willdeep-rs/bench/swebench/local-qwen-v2'
scripts = File.join(__dir__, 'scripts')
suite = JSON.parse(File.read('/Users/rocky/Sites/willdeep-react-static/public/benchmarks/taskset.json'))
meta = JSON.parse(File.read('/Users/rocky/Sites/willdeep-rs/bench/swebench/runs/cli-094rc3-glm5-60-20261009-v1/manifest.json'))
raise 'Fixed taskset mismatch' unless suite['tasks'].map { |t| t['id'] } == meta['instance_ids']
dataset = suite['tasks'].map do |task|
  dir = "#{base}/tasks/tasks/#{task.fetch('id')}"
  item = YAML.safe_load(File.read("#{dir}/task.yaml"), permitted_classes: [Time, Date])
  raise 'Fixed base mismatch' unless item['base_commit'] == task['base_commit']
  {'problem_statement' => 'problem_statement.md', 'patch' => 'gold.patch', 'test_patch' => 'test.patch', 'eval_script' => 'eval.sh', 'hints_text' => 'hints.md'}.each do |key, file|
    item[key] = File.file?("#{dir}/#{file}") ? File.read("#{dir}/#{file}") : ''
  end
  item.merge(JSON.parse(File.read("#{dir}/tests.json")))
end
binary = "#{base}/willdeep"
FileUtils.cp("#{__dir__}/container-5/willdeep", binary)
raise 'Frozen binary mismatch' unless Digest::SHA256.file(binary).hexdigest == meta['linux_binary_sha256']
%w[willdeep_swebench_60_run.rb willdeep_swebench_60_recover.rb].each do |name|
  code = File.read("#{scripts}/#{name}")
  code.gsub!('/private/tmp/willdeep-swebench-094rc3-', "#{base}/willdeep-swebench-094rc3-")
  code.gsub!('/private/tmp/willdeep-swebench-venv/bin/swebench', "#{base}/venv/bin/swebench")
  code.gsub!("#{base}/willdeep-swebench-094rc3-assets/willdeep", binary)
  code.gsub!('/private/tmp/willdeep_swebench_60_run.rb', "#{base}/willdeep_swebench_60_run.rb")
  File.write("#{base}/#{name}", code, perm: 0o600)
end
paths = Dir.glob('/Users/rocky/Sites/willdeep-rs/bench/swebench/runs/**/*') + Dir.glob('/Users/rocky/Sites/willdeep-react-static/public/benchmarks/runs/**/*')
hashes = paths.select { |p| File.file?(p) }.to_h { |p| [p, Digest::SHA256.file(p).hexdigest] }
roots = %w[qwen3.8-max qwen3.8-27b].map do |model|
  root = "#{base}/willdeep-swebench-094rc3-#{model}-60-20261010-v2"
  raise 'New run already exists' if File.exist?(root)
  FileUtils.mkdir_p(root, mode: 0o700)
  config = File.read("#{base}/source-config.toml").gsub(/^model\s*=\s*"[^"]*"/, "model = \"#{model}\"")
  File.write("#{root}/config.toml", config, perm: 0o600)
  run = meta.merge('run_id' => "cli-094rc3-#{model}-60-20261010-v2", 'model' => model, 'created_at' => Time.now.iso8601, 'paired_baseline_run_id' => nil, 'runner_version' => '0.2.1', 'runner_sha256' => Digest::SHA256.file("#{base}/willdeep_swebench_60_run.rb").hexdigest, 'binary_path' => binary, 'restart_note' => 'User authorized fresh batch after host reboot lost v1 temporary state; v1 executions excluded.')
  {'dataset.json' => dataset, 'taskset.json' => suite, 'manifest.json' => run, 'previous-report-sha256.json' => hashes, 'progress.json' => {stage: 'prepared', total: 60}}.each do |name, value|
    File.write("#{root}/#{name}", JSON.pretty_generate(value), perm: 0o600)
  end
  root
end
batch = "#{base}/batch"
FileUtils.mkdir_p(batch, mode: 0o700)
File.write("#{batch}/roots.json", JSON.generate(roots), perm: 0o600)
code = File.read("#{scripts}/willdeep_qwen38_60_batch.rb").gsub('/private/tmp/willdeep-swebench-094rc3-qwen38-batch', batch).gsub('/private/tmp/willdeep_swebench_60_run.rb', "#{base}/willdeep_swebench_60_run.rb").gsub('/private/tmp/willdeep_swebench_60_recover.rb', "#{base}/willdeep_swebench_60_recover.rb")
File.write("#{base}/willdeep_qwen38_60_batch.rb", code, perm: 0o600)
puts JSON.generate({roots: roots, protected_files: hashes.size, prepared: true})
