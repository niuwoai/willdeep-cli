require 'json'
require 'digest'
require 'fileutils'
require 'time'
source = '/private/tmp/willdeep-swebench-094rc3-glm5-60-20261009-v1'
batch = '/private/tmp/willdeep-swebench-094rc3-qwen38-batch'
raise 'Batch already exists' if File.exist?(batch)
models = %w[qwen3.8-max qwen3.8-27b]
roots = models.map { |m| "/private/tmp/willdeep-swebench-094rc3-#{m}-60-20261010-v1" }
raise 'Run already exists' if roots.any? { |r| File.exist?(r) }
paths = Dir.glob('/Users/rocky/Sites/willdeep-rs/bench/swebench/runs/**/*') + Dir.glob('/Users/rocky/Sites/willdeep-react-static/public/benchmarks/runs/**/*')
hashes = paths.select { |p| File.file?(p) }.to_h { |p| [p, Digest::SHA256.file(p).hexdigest] }
roots.zip(models).each do |root, model|
  FileUtils.mkdir_p(root, mode: 0o700)
  %w[dataset.json taskset.json].each { |f| FileUtils.cp("#{source}/#{f}", "#{root}/#{f}") }
  config = File.read("#{source}/config.toml").gsub(/^model\s*=\s*"[^"]*"/, "model = \"#{model}\"")
  File.write("#{root}/config.toml", config, perm: 0o600)
  meta = JSON.parse(File.read("#{source}/manifest.json"))
  meta.merge!('run_id' => "cli-094rc3-#{model}-60-20261010-v1", 'model' => model, 'created_at' => Time.now.iso8601, 'paired_baseline_run_id' => nil)
  File.write("#{root}/manifest.json", JSON.pretty_generate(meta), perm: 0o600)
  File.write("#{root}/previous-report-sha256.json", JSON.pretty_generate(hashes), perm: 0o600)
  File.write("#{root}/progress.json", JSON.generate({stage: 'prepared', total: 60}), perm: 0o600)
end
FileUtils.mkdir_p(batch, mode: 0o700)
File.write("#{batch}/roots.json", JSON.generate(roots), perm: 0o600)
puts JSON.generate({roots: roots, protected_files: hashes.size})
