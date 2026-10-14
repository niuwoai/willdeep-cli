#!/usr/bin/env ruby
require 'json'
require 'open3'
require 'fileutils'
require 'time'

root = File.expand_path('..', __dir__)
output = ARGV.fetch(0, File.join(root, 'target', 'swebench-runtime-hardening-validation'))
FileUtils.mkdir_p(output)
gates = [
  ['format', root, %w[cargo fmt --all --check]],
  ['clippy', root, %w[cargo clippy --workspace --all-targets -- -D warnings]],
  ['workspace-tests', root, %w[cargo test --workspace]],
  ['web-lint', File.join(root, 'web'), %w[yarn lint]],
  ['web-build', File.join(root, 'web'), %w[yarn build]],
  ['source-size', root, ['ruby', 'scripts/check_source_size.rb', '--output', File.join(output, 'source-size')]],
  ['diff', root, %w[git diff --check]]
]
report = { version: JSON.parse(File.read(File.join(root, 'web/package.json')))['version'], started_at: Time.now.iso8601, gates: [] }
def save_report(output, report)
  File.write(File.join(output, 'report.json'), JSON.pretty_generate(report))
  lines = ['# SWE-bench 运行时强化验证', '', "候选版本：#{report[:version]}", '', '|检查|状态|秒数|', '|---|---|---|']
  report[:gates].each { |g| lines << "|#{g[:name]}|#{g[:exit_code].zero? ? '通过' : '失败'}|#{g[:seconds]}|" }
  lines << '' << '这是机制与工程回归，不是新的 SWE-bench 模型成绩。真实费用及解决率提升未经本次测试证明。'
  File.write(File.join(output, 'report.md'), lines.join("\n") + "\n")
end
gates.each do |name, directory, argv|
  puts JSON.generate(stage: name, state: 'running')
  started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  log = File.join(output, "#{name}.log")
  status = File.open(log, 'w', 0o600) do |file|
    pid = Process.spawn(*argv, chdir: directory, out: file, err: file)
    Process.wait2(pid).last
  end
  elapsed = (Process.clock_gettime(Process::CLOCK_MONOTONIC) - started).round(2)
  summaries = File.readlines(log).filter { |line| line.start_with?('test result:') }.map(&:strip)
  gate = { name: name, argv: argv, exit_code: status.exitstatus || 128, seconds: elapsed, test_summaries: summaries }
  report[:gates] << gate
  save_report(output, report)
  puts JSON.generate(gate)
  unless status.success?
    report[:state] = 'failed'
    save_report(output, report)
    exit 1
  end
end
report[:state] = 'passed'
report[:finished_at] = Time.now.iso8601
save_report(output, report)
