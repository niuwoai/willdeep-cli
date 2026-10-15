# frozen_string_literal: true
require 'optparse'
require_relative 'agent_rsi_reproduction'

options = {}
OptionParser.new do |parser|
  parser.on('--task ID') { |value| options[:task] = value }
  parser.on('--report PATH') { |value| options[:report] = value }
  parser.on('--evidence PATH') { |value| options[:evidence] = value }
  parser.on('--output-prefix PATH') { |value| options[:output] = value }
end.parse!
abort 'task, report, evidence and output-prefix required' unless options.values_at(:task, :report, :evidence, :output).all? && ARGV.empty?
raw = (File.open(options[:report], 'r:UTF-8') { |file| file.read(AgentRsiReproduction::MAX_BYTES + 1) } || '').dup.force_encoding(Encoding::UTF_8)
evidence_raw = (File.open(options[:evidence], 'r:UTF-8') { |file| file.read(AgentRsiReproduction::MAX_BYTES + 1) } || '').dup.force_encoding(Encoding::UTF_8)
abort 'host evidence exceeds size limit' if evidence_raw.bytesize > AgentRsiReproduction::MAX_BYTES
abort 'host evidence is not valid UTF-8' unless evidence_raw.valid_encoding?
evidence = JSON.parse(evidence_raw)
result = AgentRsiReproduction.grade(raw, task_id: options[:task], evidence: evidence)
json = JSON.pretty_generate(result) + "\n"
markdown = "# 公共免费复现验收\n\ncase：#{result['case_id']}；格式 #{result['format_valid']}；分类 #{result['classification_correct'].inspect}；定位 #{result['line_correct'].inspect}；证据 #{result['evidence_verified']}；复现 #{result['reproduction_status']}；整体通过 #{result['passed']}。\n\n宿主执行 #{result['objective_execution_count']} 次；模型调用 0、费用 0。干净题复现字段明确 N/A；不评价 reason 语义，不构成质量提升或晋升结论。\n"
%w[json md].zip([json, markdown]).each do |extension, body|
  File.open(options[:output] + '.' + extension, File::WRONLY | File::CREAT | File::EXCL, 0o600) { |file| file.write(body) }
end
puts JSON.generate(result.slice('case_id', 'format_valid', 'classification_correct', 'line_correct', 'evidence_verified', 'reproduction_status', 'passed', 'objective_execution_count', 'error_code'))
