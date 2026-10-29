require 'json'
require 'time'
require 'open3'
BATCH = '/private/tmp/willdeep-swebench-094rc3-qwen38-batch'
ROOTS = JSON.parse(File.read("#{BATCH}/roots.json"))
File.write("#{BATCH}/jobs.json", JSON.generate({pid: Process.pid, script: __FILE__, roots: ROOTS, started_at: Time.now.iso8601}))
def status(stage, extra = {})
  File.write("#{BATCH}/status.json", JSON.pretty_generate({stage: stage, at: Time.now.iso8601}.merge(extra)))
end
ROOTS.each do |root|
  log = File.open("#{root}/runner.log", 'a', 0o600)
  unless File.file?("#{root}/control-passed.json")
    status('environment_control', root: root)
    ok = system('ruby', '/private/tmp/willdeep_swebench_60_run.rb', root, '--control', out: log, err: log)
    unless ok
      status('blocked', root: root, reason: 'Gold environment control failed; model execution not started')
      exit 1
    end
    File.write("#{root}/control-passed.json", JSON.generate({at: Time.now.iso8601}))
  end
  status('running', root: root)
  system('ruby', '/private/tmp/willdeep_swebench_60_run.rb', root, out: log, err: log)
  report = JSON.parse(File.read("#{root}/summary.json"))
  if report['model_executed'] != 60 || report['instances'].any? { |r| %w[infrastructure_error interrupted].include?(r['status']) }
    status('blocked', root: root, reason: 'Preparation or unknown execution failure; never rerun executed answers')
    exit 1
  end
  system('ruby', '/private/tmp/willdeep_swebench_60_recover.rb', root, out: log, err: log) unless report['completed'] == 60
  log.close
end
reports = ROOTS.map { |r| JSON.parse(File.read("#{r}/summary.json")) }
status(reports.all? { |r| r['completed'] == 60 } ? 'complete' : 'blocked', reports: reports.map { |r| r.slice('run_id', 'completed', 'model_executed', 'resolved', 'statuses') })
