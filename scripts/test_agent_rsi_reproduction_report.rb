# frozen_string_literal: true
require 'json'
require 'digest'
require 'open3'
require 'time'
require_relative 'lib/agent_rsi_reproduction'

root = AgentRsiReproduction::ROOT
destination = AgentRsiReproduction::FIXTURE_ROOT
test = File.join(root, 'scripts/test_agent_rsi_reproduction.rb')
log, status = Open3.capture2e('ruby', test)
abort log unless status.success?
counts = log.match(/(\d+) runs, (\d+) assertions, (\d+) failures, (\d+) errors, (\d+) skips/)
abort 'missing test summary' unless counts
manifest = JSON.parse(File.read(File.join(destination, 'manifest.json')))
rows = manifest.fetch('tasks').map do |task|
  raw = JSON.generate('has_defect' => task['has_defect'], 'line' => task['defect_line'],
                      'reason' => 'Public engineering fixture check.', 'reproduction' => task['public_witness'])
  evidence = {'fixture_sha256' => task['fixture_sha256'], 'read_observed' => true,
              'delivered_to_model' => true, 'fixture_unchanged' => true}
  AgentRsiReproduction.grade(raw, task_id: task['case_id'], evidence: evidence)
end
abort 'public execution did not pass' unless rows.all? { |row| row['passed'] }
files = %w[scripts/lib/agent_rsi_reproduction.rb scripts/lib/agent_rsi_reproduction_eval.rb scripts/lib/agent_rsi_reproduction_harness.js scripts/test_agent_rsi_reproduction.rb bench/agent-rsi/reproduction-free-v1/manifest.json bench/agent-rsi/review-output-v2/contract.json bench/agent-rsi/review-output-v2/golden.json]
report = {'schema' => 'willdeep.agent-rsi.reproduction-free-verification.v1', 'created_at' => Time.now.utc.iso8601,
          'model_calls' => 0, 'cost_microusd' => 0, 'network_calls' => 0, 'paid_or_legacy_trials_modified' => false,
          'public_engineering_fixture_only' => true, 'quality_improvement_claim' => false, 'automatic_promotion' => false,
          'tests' => %w[runs assertions failures errors skips].zip(counts.captures.map(&:to_i)).to_h,
          'test_output' => log, 'public_cases' => rows,
          'actual_host_function_executions' => rows.sum { |row| row['objective_execution_count'] },
          'sha256' => files.to_h { |path| [path, Digest::SHA256.file(File.join(root, path)).hexdigest] }}
File.write(File.join(destination, 'free-verification.json'), JSON.pretty_generate(report) + "\n")
table = rows.map { |row| "| #{row['case_id']} | #{row['format_valid']} | #{row['classification_correct']} | #{row['line_correct']} | #{row['evidence_verified']} | #{row['reproduction_status']} | #{row['objective_execution_count']} | #{row['passed']} |" }
markdown = [
  '# 公共免费复现评分：工程验收', '',
  "#{report['tests']['runs']} 项测试、#{report['tests']['assertions']} 断言通过；#{report['tests']['failures']} 失败、#{report['tests']['errors']} 错误。六个公开案例实际执行可信 fixture/reference 共 #{report['actual_host_function_executions']} 次。", '',
  '| 案例 | 格式 | 分类 | 定位 | 证据 | 复现 | 宿主函数执行数 | 通过 |', '|---|---|---|---|---|---|---|---|', *table, '',
  '反例覆盖：伪造 expected/actual、未触发缺陷、无复现、错误行、缺读取/未交付/文件修改、额外代码与路径、重复键、float/exponent/-0、原型键、深度/数量/字符串/字节超限、非白名单源码、fixture/reference 篡改、符号链接越界、可信无限循环超时、过大输出及无 process/require 沙箱边界。', '',
  '宿主证据与模型报告分开；真实执行结果与模型声明逐项核对。clean 的复现维度是 N/A，公开探针通过不代表证明所有输入。reason 仅校验类型和非空，不做语义 judge。', '',
  '模型调用 0、费用 0、网络调用 0；旧付费批次/旧 grader/旧分数保持不动。该报告只证明免费工程链路，不宣称模型质量改善或版本晋升。', ''
].join("\n")
File.write(File.join(destination, 'free-verification.md'), markdown)
puts JSON.generate(report.slice('tests', 'actual_host_function_executions', 'model_calls', 'cost_microusd'))
