require 'json'
require 'fileutils'
require 'time'
require '/Users/rocky/Sites/willdeep-rs/scripts/lib/agent_eval_process'
root = File.expand_path(ARGV.fetch(0))
raise 'Unexpected run root' unless root.start_with?('/private/tmp/willdeep-swebench-094rc3-')
meta = JSON.parse(File.read("#{root}/manifest.json"))
rows = JSON.parse(File.read("#{root}/summary.json")).fetch('instances')
pending = rows.select { |r| r['status'] == 'evaluation_error' }
def evaluate(root, id, run_id, prediction = nil)
  args = ['/private/tmp/willdeep-swebench-venv/bin/swebench', 'eval', "#{root}/dataset.json", '--run-id', run_id, '-i', id, '-j', '1', '-t', '1200']
  args += prediction ? ['-p', prediction] : ['--gold']
  logs = "#{root}/recovery/#{run_id}"
  FileUtils.mkdir_p(logs, mode: 0o700)
  code, timeout, = AgentEvalProcess.run(args, {}, root, 1500, logs)
  raise 'Official evaluator failed or exceeded unchanged time limit' unless code == 0 && !timeout
  result = JSON.parse(File.read("#{root}/logs/evaluation/#{run_id}/results.json"))
  raise 'Official evaluator did not complete' unless result.fetch('completed_ids').include?(id)
  result
end
pending.each do |row|
  id = row.fetch('instance_id')
  slot = "#{root}/recovery/#{id}"
  next if File.file?("#{slot}/attempt.json")
  FileUtils.mkdir_p(slot, mode: 0o700)
  File.write("#{slot}/attempt.json", JSON.generate({pid: Process.pid, at: Time.now.iso8601, model_not_rerun: true, timeout_seconds: 1200}))
  begin
    File.write("#{root}/progress.json", JSON.generate({stage: 'environment_control', instance_id: id, total: 60}))
    control = evaluate(root, id, "#{meta['run_id']}-#{id}-environment-control")
    raise 'Reference patch did not resolve in environment control' unless control.fetch('resolved_ids').include?(id)
    run_id = "#{meta['run_id']}-#{id}"
    existing = "#{root}/logs/evaluation/#{run_id}"
    FileUtils.mv(existing, "#{slot}/original-evaluation") if File.directory?(existing)
    File.write("#{root}/progress.json", JSON.generate({stage: 'retry_evaluation', instance_id: id, total: 60}))
    evaluate(root, id, run_id, "#{root}/instances/#{id}/prediction.jsonl")
    official = JSON.parse(File.read("#{root}/logs/evaluation/#{run_id}/willdeep-#{meta['binary_version']}-#{meta['model']}/#{id}/report.json"))
    verdict = official.fetch(id).fetch('resolved')
    raise 'Invalid verdict' unless [true, false].include?(verdict)
    row.merge!('resolved' => verdict, 'status' => verdict ? 'resolved' : 'unresolved', 'finished_at' => Time.now.iso8601, 'grading_recovery' => 'Same evaluator and 1200-second limit after successful reference-patch control')
    File.write("#{root}/instances/#{id}/status.json", JSON.pretty_generate(row))
  rescue => error
    File.write("#{slot}/failure.json", JSON.pretty_generate({error: error.message, at: Time.now.iso8601, model_not_rerun: true, timeout_unchanged: true}))
  end
end
report = JSON.parse(File.read("#{root}/summary.json"))
rows = Dir.glob("#{root}/instances/*/status.json").map { |p| JSON.parse(File.read(p)) }
report.merge!('instances' => rows, 'resolved' => rows.count { |r| r['resolved'] == true }, 'completed' => rows.count { |r| r.key?('agent_exit') && [true, false].include?(r['resolved']) }, 'updated_at' => Time.now.iso8601, 'statuses' => rows.group_by { |r| r['status'] }.transform_values(&:size))
report['resolved_rate_over_planned'] = report['resolved'] / 60.0
File.write("#{root}/summary.json", JSON.pretty_generate(report))
File.write("#{root}/progress.json", JSON.pretty_generate({stage: report['completed'] == 60 ? 'complete' : 'blocked', total: 60, pending_ids: rows.select { |r| r['resolved'].nil? }.map { |r| r['instance_id'] }}))
exit(report['completed'] == 60 ? 0 : 1)
