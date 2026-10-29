require 'json'
require 'time'
require 'fileutils'
ROOTS = %w[deepseek-flash glm5].map { |model| "/private/tmp/willdeep-swebench-094rc3-#{model}-60-20261009-v1" }
BATCH = '/private/tmp/willdeep-swebench-094rc3-batch'
FileUtils.mkdir_p(BATCH, mode: 0o700)
File.write("#{BATCH}/jobs.json", JSON.pretty_generate({pid: Process.pid, script: '/private/tmp/willdeep_swebench_60_batch.rb', started_at: Time.now.iso8601, roots: ROOTS}))
control = JSON.parse(File.read("#{BATCH}/control-job.json"))
control_result = "#{ROOTS.first}/logs/evaluation/cli-094rc3-deepseek-flash-60-20261009-v1-gold-control/results.json"
loop do
  if File.file?(control_result)
    result = JSON.parse(File.read(control_result))
    raise 'Environment control did not resolve reference patch' unless result.fetch('resolved_ids').include?('matplotlib__matplotlib-14623')
    break
  end
  command = IO.popen(['ps', '-p', control.fetch('pid').to_s, '-o', 'command='], &:read).strip
  unless command.end_with?('/private/tmp/willdeep_swebench_60_run.rb ' + ROOTS.first + ' --control')
    File.write("#{BATCH}/status.json", JSON.pretty_generate({stage: 'blocked', reason: 'Reference environment control exited without valid result; no model answers executed', at: Time.now.iso8601}))
    exit 1
  end
  File.write("#{BATCH}/status.json", JSON.pretty_generate({stage: 'awaiting_environment_control', at: Time.now.iso8601}))
  sleep 5
end
ROOTS.each do |root|
  log = File.open("#{root}/runner.log", 'a', 0o600)
  system('ruby', '/private/tmp/willdeep_swebench_60_run.rb', root, out: log, err: log)
  report = JSON.parse(File.read("#{root}/summary.json"))
  if report['model_executed'] != 60 || report['instances'].any? { |row| %w[infrastructure_error interrupted].include?(row['status']) }
    File.write("#{BATCH}/status.json", JSON.pretty_generate({stage: 'blocked', root: root, reason: 'Preparation or unknown execution failure; never rerun an executed answer automatically', at: Time.now.iso8601}))
    exit 1
  end
  system('ruby', '/private/tmp/willdeep_swebench_60_recover.rb', root, out: log, err: log) unless report['completed'] == 60
  log.close
end
reports = ROOTS.map { |root| JSON.parse(File.read("#{root}/summary.json")) }
File.write("#{BATCH}/status.json", JSON.pretty_generate({stage: reports.all? { |r| r['completed'] == 60 } ? 'complete' : 'blocked', at: Time.now.iso8601, reports: reports.map { |r| r.slice('run_id', 'completed', 'model_executed', 'resolved', 'statuses') }}))
