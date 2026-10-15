#!/usr/bin/env ruby
# frozen_string_literal: true

# Free engineering acceptance only. Never builds a real provider or promotes an agent.
require 'json'
require 'optparse'
require 'open3'
require 'fileutils'
require 'digest'
require_relative 'lib/prompt_rsi/gate'

module AgentRsiEval
  module_function

  def run(options)
    required = %i[binary home baseline candidate id out]
    abort("Missing: #{required.reject { |key| options[key] }.join(', ')}") unless required.all? { |key| options[key] }
    binary = File.expand_path(options[:binary])
    home = File.expand_path(options[:home])
    environment = {'WILLDEEP_HOME' => home}
    command = [binary, 'agents', 'rsi', 'prepare', '--baseline', options[:baseline], '--candidate', options[:candidate], '--id', options[:id]]
    command += ['--suite', options[:suite]] if options[:suite]
    capture(environment, command) unless options[:resume]
    candidate = JSON.parse(File.read(options[:candidate]))
    command = [binary, 'agents', 'rsi', 'run', options[:id], '--agent-revision', candidate.fetch('sha256')]
    command << '--resume' if options[:resume]
    report = capture(environment, command)
    rows = report.fetch('evidence').map do |row|
      {'task' => "#{row.fetch('task_id')}:#{row.fetch('repetition')}:#{row.fetch('attempt')}", 'arm' => row.fetch('arm'),
       'split' => row.fetch('split'), 'status' => normalize_status(row.fetch('status')),
       'input_tokens' => row.dig('usage', 'input_tokens'), 'output_tokens' => row.dig('usage', 'output_tokens'),
       'elapsed_seconds' => row.dig('usage', 'elapsed_ms')&.fdiv(1000)}
    end
    # Reuse existing measured-resource/statistics semantics; unknown usage stays unknown.
    stats = %w[baseline candidate].to_h { |arm| [arm, PromptRsi::Gate.stats(rows.select { |row| row['arm'] == arm })] }
    report['prompt_rsi_resource_summary'] = stats
    report['binary_sha256'] = Digest::SHA256.file(binary).hexdigest
    FileUtils.mkdir_p(options[:out])
    File.write(File.join(options[:out], 'report.json'), JSON.pretty_generate(report) + "\n")
    File.write(File.join(options[:out], 'report.md'), markdown(report))
    puts JSON.generate(report.slice('experiment_id', 'verdict', 'independent_samples', 'attempts', 'cost_microusd', 'quality_improvement_claimed'))
    report
  end

  def capture(environment, command)
    output, _error, status = Open3.capture3(environment, *command)
    abort("Offline RSI command failed (#{status.exitstatus}); no real model was requested") unless status.success?
    JSON.parse(output)
  end

  def normalize_status(status)
    {'infra_error' => 'error', 'interrupted' => 'skipped', 'invalid' => 'error', 'unverified' => 'skipped', 'budget_exhausted' => 'timeout'}.fetch(status, status)
  end

  def markdown(report)
    "# RSI 免费离线验收\n\n实验：#{report['experiment_id']}\n\n结论：#{report['verdict']}\n\n独立配对样本：#{report['independent_samples']}；尝试：#{report['attempts']}；实际费用：0。\n\n真实 Worker / 文件工具已执行；模型为确定性 Mock。此报告不代表模型质量提升，不可用于官方晋升。\n\n资源统计复用 Prompt RSI 的缺测口径；完整血缘、全部尝试和恢复关联见同名 JSON。\n"
  end
end

if $PROGRAM_NAME == __FILE__
  options = {}
  OptionParser.new do |parser|
    %i[binary home baseline candidate id suite out].each { |key| parser.on("--#{key} VALUE") { |value| options[key] = value } }
    parser.on('--resume') { options[:resume] = true }
  end.parse!
  AgentRsiEval.run(options)
end
