require 'json'
require 'fileutils'
require 'open3'
require 'digest'
require 'time'
require '/Users/rocky/Sites/willdeep-rs/scripts/lib/agent_eval_process'

ROOT = '/private/tmp/willdeep-swebench-glm5-30-20261009'
BINARY = '/private/tmp/willdeep-swebench-venv/willdeep'
SWEB = '/private/tmp/willdeep-swebench-venv/bin/swebench'
DATA = JSON.parse(File.read("#{ROOT}/dataset.json"))
MANIFEST = JSON.parse(File.read("#{ROOT}/manifest.json"))
RUN = 'willdeep-092rc1-glm5-30-20261009-v1'
$stdout.sync = true

def command(argv, logs, seconds = 180)
  FileUtils.mkdir_p(logs, mode: 0o700)
  AgentEvalProcess.run(argv, {}, ROOT, seconds, logs)
end

def checked(argv, logs, seconds = 180)
  code, timeout, elapsed = command(argv, logs, seconds)
  raise "command failed (exit=#{code.inspect}, timeout=#{timeout}); see private logs #{logs}" unless code == 0 && !timeout
  elapsed
end

def docker_capture(*args)
  output, status = Open3.capture2('docker', *args)
  raise "Docker readback failed: #{args.first}" unless status.success?
  output
end

def pull_image(image, slot)
  4.times do |attempt|
    code, timed_out, = command(['docker', 'pull', '--platform', 'linux/amd64', image], "#{slot}/pull-attempt-#{attempt + 1}", 1200)
    return if code == 0 && !timed_out
    puts "image download failed; retry #{attempt + 1}/4"
    sleep 30 if attempt < 3
  end
  raise 'image download failed after four attempts; batch paused for infrastructure recovery'
end

def run_agent(argv, name, slot)
  logs = "#{slot}/agent"
  FileUtils.mkdir_p(logs, mode: 0o700)
  last_check = 0
  tokens = 0
  stopped = false
  guard = lambda do
    now = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    next false if now - last_check < 2
    last_check = now
    ledger = "#{slot}/budget-usage.jsonl"
    month = Time.now.utc.strftime('%Y-%m')
    _, _, status = Open3.capture3('docker', 'cp', "#{name}:/opt/willdeep-state/usage/#{month}.jsonl", ledger)
    next false unless status.success?
    records = File.readlines(ledger).filter_map { |line| JSON.parse(line) rescue nil }
    records.uniq! { |record| record.fetch('id') }
    tokens = records.sum { |record| record['total_tokens'] || record['input_tokens'].to_i + record['output_tokens'].to_i }
    next false if tokens < 1_000_000
    puts "budget stop: #{File.basename(slot)} all-agent reported tokens=#{tokens}"
    _, _, status = Open3.capture3('docker', 'stop', '-t', '2', name)
    raise 'budget guard could not stop task container' unless status.success?
    stopped = true
    true
  end
  result = AgentEvalProcess.run(argv, {}, ROOT, 1250, logs, interrupt_when: guard)
  [*result, tokens, stopped]
end

def summary
  rows = Dir.glob("#{ROOT}/instances/*/status.json").map { |f| JSON.parse(File.read(f)) }
  resolved = rows.count { |r| r['resolved'] == true }
  executed = rows.count { |r| r.key?('agent_exit') }
  completed = rows.count { |r| r.key?('agent_exit') && !%w[infrastructure_error evaluation_error interrupted].include?(r['status']) }
  result = {
    run_id: RUN, updated_at: Time.now.iso8601, planned: 30, completed: completed, model_executed: executed, recorded_statuses: rows.size,
    resolved: resolved, resolved_rate_over_planned: resolved / 30.0,
    statuses: rows.group_by { |r| r['status'] }.transform_values(&:size),
    model: MANIFEST['model'], binary_version: MANIFEST['binary_version'], instances: rows,
    usage: usage_summary, cost: nil, cost_note: 'Provider usage ledger has tokens but no billing amount; unknown cost is not zero.'
  }
  File.write("#{ROOT}/summary.json", JSON.pretty_generate(result))
  lines = ["# WillDeep SWE-bench Verified 30 题评测", '',
           "模型：`#{MANIFEST['model']}`；WillDeep：`#{MANIFEST['binary_version']}`。", '',
           "有效终态 #{completed}/30；已执行模型 #{executed}/30；官方判定解决 #{resolved}/30。基础设施准备失败和待裁判不算完成，未完成时该比率不是最终成绩。", '',
           '| 任务 | 状态 | 官方解决 | Agent 秒数 |', '|---|---|---|---|']
  rows.each { |r| lines << "| #{r['instance_id']} | #{r['status']} | #{r['resolved'].inspect} | #{r['agent_seconds']} |" }
  File.write("#{ROOT}/summary.md", lines.join("\n") + "\n")
  puts JSON.generate(result.reject { |k, _| k == :instances })
end

def usage_summary
  records = Dir.glob("#{ROOT}/instances/*/collected/usage/*.jsonl").flat_map do |path|
    File.readlines(path).filter_map { |line| JSON.parse(line) rescue nil }
  end
  result = {model_calls: records.size, models: records.group_by { |r| r['model'] }.transform_values(&:size)}
  %w[input_tokens output_tokens cache_read_tokens total_tokens].each do |key|
    values = records.filter_map { |r| r[key] }
    result[key] = values.empty? ? nil : values.sum
    result["#{key}_known_calls"] = values.size
  end
  result
end

def grade(task, slot)
  id = task.fetch('instance_id')
  run_id = "#{RUN}-#{id}"
  code, timed_out, = command([SWEB, 'eval', "#{ROOT}/dataset.json", '-p', "#{slot}/prediction.jsonl",
                             '--run-id', run_id, '-i', id, '-j', '1', '-t', '1200'], "#{slot}/evaluation", 1500)
  report = "#{ROOT}/logs/evaluation/#{run_id}/willdeep-#{MANIFEST['binary_version']}-#{MANIFEST['model']}/#{id}/report.json"
  return [nil, 'evaluation_error'] if timed_out || code != 0 || !File.file?(report)
  verdict = JSON.parse(File.read(report)).fetch(id).fetch('resolved')
  [verdict, verdict ? 'resolved' : 'unresolved']
end

if ARGV.include?('--control')
  task = DATA.first
  id = task.fetch('instance_id')
  checked([SWEB, 'eval', "#{ROOT}/dataset.json", '--gold', '--run-id', "#{RUN}-gold-control",
           '-i', id, '-j', '1', '-t', '1200'], "#{ROOT}/control", 1800)
  report = JSON.parse(File.read("#{ROOT}/logs/evaluation/#{RUN}-gold-control/results.json"))
  raise 'gold control did not resolve' unless report.fetch('resolved_ids').include?(id)
  puts "gold control passed: #{id}"
  exit
end

if ARGV.include?('--guard-smoke')
  name = 'willdeep-glm5-budget-smoke'
  slot = "#{ROOT}/guard-smoke"
  FileUtils.mkdir_p(slot, mode: 0o700)
  fixture = {id: 'guard-smoke', kind: 'subagent', input_tokens: 1_000_001, output_tokens: 1, total_tokens: 1_000_002}
  File.write("#{slot}/fixture.jsonl", JSON.generate(fixture) + "\n")
  begin
    checked(['docker', 'create', '--name', name, 'alpine:3.20', 'sleep', 'infinity'], "#{slot}/create")
    checked(['docker', 'start', name], "#{slot}/start")
    checked(['docker', 'exec', name, 'mkdir', '-p', '/opt/willdeep-state/usage'], "#{slot}/mkdir")
    checked(['docker', 'cp', "#{slot}/fixture.jsonl", "#{name}:/opt/willdeep-state/usage/#{Time.now.utc.strftime('%Y-%m')}.jsonl"], "#{slot}/copy")
    result = run_agent(['docker', 'exec', name, 'sleep', '60'], name, slot)
    raise 'guard failed to interrupt above-limit subagent usage' unless result[3] && result[5]
    checked(['docker', 'start', name], "#{slot}/restart")
    checked(['docker', 'exec', name, 'test', '-f', "/opt/willdeep-state/usage/#{Time.now.utc.strftime('%Y-%m')}.jsonl"], "#{slot}/state-intact")
    File.write("#{slot}/result.json", JSON.pretty_generate({passed: true, subagent_tokens_included: true, reported_tokens: result[4], state_preserved_after_restart: true}))
    puts 'budget guard smoke passed'
  ensure
    command(['docker', 'rm', '-f', name], "#{slot}/cleanup", 60)
  end
  exit
end

summary
DATA.each_with_index do |task, index|
  id = task.fetch('instance_id')
  slot = "#{ROOT}/instances/#{id}"
  FileUtils.mkdir_p(slot, mode: 0o700)
  if File.file?("#{slot}/status.json")
    previous = JSON.parse(File.read("#{slot}/status.json"))
    next if previous['status'] == 'evaluation_error' && File.file?("#{slot}/deferred-grading.json")
    if previous['status'] == 'evaluation_error' && File.file?("#{slot}/prediction.jsonl")
      File.write("#{ROOT}/progress.json", JSON.pretty_generate({index: index + 1, total: 30, instance_id: id, stage: 'retry_evaluation'}))
      pull_image(task.fetch('image'), slot)
      verdict, status = grade(task, slot)
      previous.merge!('resolved' => verdict, 'status' => status, 'finished_at' => Time.now.iso8601)
      File.write("#{slot}/status.json", JSON.pretty_generate(previous))
      summary
      if verdict.nil?
        File.write("#{ROOT}/progress.json", JSON.pretty_generate({stage: 'blocked', instance_id: id, reason: 'official evaluator unavailable; patch preserved'}))
        abort 'Official evaluator unavailable; preserve patch and retry grading only.'
      end
    end
    next if previous.key?('agent_exit')
    archive = "#{ROOT}/infrastructure-attempts/#{id}/#{Time.now.to_i}"
    FileUtils.mkdir_p(File.dirname(archive), mode: 0o700)
    FileUtils.mv(slot, archive)
    FileUtils.mkdir_p(slot, mode: 0o700)
  end
  name = "willdeep-glm5-sweb30-#{index}"
  row = {instance_id: id, repo: task['repo'], started_at: Time.now.iso8601, resolved: nil}
  File.write("#{ROOT}/progress.json", JSON.pretty_generate({index: index + 1, total: 30, instance_id: id, stage: 'prepare'}))
  puts "[#{index + 1}/30] #{id} prepare"
  begin
    image = task.fetch('image')
    pull_image(image, slot)
    row[:image_id] = docker_capture('image', 'inspect', '--format', '{{.Id}}', image).strip
    checked(['docker', 'create', '--platform', 'linux/amd64', '--name', name, '--user', '0', '--memory', '3g',
             '--cpus', '2', '--pids-limit', '256', '--cap-drop', 'ALL', '--cap-add', 'DAC_OVERRIDE', '--security-opt', 'no-new-privileges',
             image, 'sleep', 'infinity'], "#{slot}/create")
    checked(['docker', 'start', name], "#{slot}/start")
    checked(['docker', 'exec', name, 'mkdir', '-p', '/opt/willdeep-state'], "#{slot}/state")
    checked(['docker', 'cp', BINARY, "#{name}:/opt/willdeep"], "#{slot}/copy-binary")
    checked(['docker', 'exec', name, '/opt/willdeep', '--version'], "#{slot}/binary-version")
    checked(['docker', 'cp', "#{ROOT}/config.toml", "#{name}:/opt/willdeep-state/config.toml"], "#{slot}/copy-config")
    checked(['docker', 'exec', name, 'git', '-C', '/testbed', 'reset', '--hard', task.fetch('base_commit')], "#{slot}/reset-base")
    checked(['docker', 'exec', name, 'git', '-C', '/testbed', 'checkout', '--detach', task.fetch('base_commit')], "#{slot}/detach-base")
    # Official images may retain an untracked build directory from installation.
    # Clean only this fresh task container, before the model has started.
    checked(['docker', 'exec', name, 'git', '-C', '/testbed', 'clean', '-fd'], "#{slot}/clean-image-residue")
    refs = docker_capture('exec', name, 'git', '-C', '/testbed', 'for-each-ref', '--format=%(refname)').lines.map(&:strip)
    refs.each_with_index do |ref, number|
      checked(['docker', 'exec', name, 'git', '-C', '/testbed', 'update-ref', '-d', ref], "#{slot}/delete-ref-#{number}")
    end
    checked(['docker', 'exec', name, 'git', '-C', '/testbed', 'reflog', 'expire', '--expire=now', '--all'], "#{slot}/expire-future")
    checked(['docker', 'exec', name, 'git', '-C', '/testbed', 'gc', '--prune=now'], "#{slot}/prune-future", 300)
    base = docker_capture('exec', name, 'git', '-C', '/testbed', 'rev-parse', 'HEAD').strip
    raise 'image has incorrect base commit' unless base == task.fetch('base_commit')
    raise 'image workspace is dirty' unless docker_capture('exec', name, 'git', '-C', '/testbed', 'status', '--porcelain').strip.empty?
    prompt = "You are fixing a real repository issue. Work in /testbed. Resolve the issue below by editing production code and running relevant tests. Do not change or delete existing tests, fetch reference solutions, browse the internet for this issue, access future commits, or commit changes. The testbed conda environment is already activated for shell commands. Leave your final changes in the working tree.\n\n#{task.fetch('problem_statement')}\n"
    File.write("#{slot}/prompt.md", prompt, mode: 'w', perm: 0o600)
    checked(['docker', 'cp', "#{slot}/prompt.md", "#{name}:/opt/task.md"], "#{slot}/copy-prompt")
    File.write("#{ROOT}/progress.json", JSON.pretty_generate({index: index + 1, total: 30, instance_id: id, stage: 'agent'}))
    puts "[#{index + 1}/30] #{id} agent"
    shell = 'source /opt/miniconda3/bin/activate testbed && exec timeout --kill-after=10s 1200 /opt/willdeep --config /opt/willdeep-state/config.toml --workspace /testbed --full-auto --max-turns 60 --model glm-5 run --local --output json --input /opt/task.md'
    code, timeout, elapsed, interrupted, reported_tokens, budget_stopped = run_agent(
      ['docker', 'exec', '-e', 'WILLDEEP_HOME=/opt/willdeep-state', '-w', '/testbed', name, 'bash', '-lc', shell], name, slot)
    checked(['docker', 'start', name], "#{slot}/resume-for-collection") if budget_stopped
    raise 'host timeout; container execution state unknown' if timeout
    timeout = [124, 137].include?(code)
    row.merge!(agent_exit: code, agent_timeout: timeout, agent_seconds: elapsed.round(1),
               all_agent_budget_stopped: budget_stopped, budget_last_observed_tokens: reported_tokens)
    checked(['docker', 'exec', name, 'git', '-C', '/testbed', 'add', '-N', '.'], "#{slot}/include-new")
    patch = docker_capture('exec', name, 'git', '-C', '/testbed', 'diff', '--binary', base)
    File.write("#{slot}/patch.diff", patch, mode: 'w', perm: 0o600)
    prediction = {instance_id: id, model_name_or_path: "willdeep-#{MANIFEST['binary_version']}-#{MANIFEST['model']}", model_patch: patch}
    File.write("#{slot}/prediction.jsonl", JSON.generate(prediction) + "\n", mode: 'w', perm: 0o600)
    command(['docker', 'cp', "#{name}:/opt/willdeep-state", "#{slot}/collected"], "#{slot}/collect-state")
    checked(['docker', 'rm', '-f', name], "#{slot}/remove")
    if patch.empty?
      row.merge!(resolved: false, status: budget_stopped ? 'budget_exhausted_empty_patch' : ([1, 2, 3, 127].include?(code) ? 'agent_error' : 'empty_patch'))
    else
      File.write("#{ROOT}/progress.json", JSON.pretty_generate({index: index + 1, total: 30, instance_id: id, stage: 'evaluation'}))
      verdict, status = grade(task, slot)
      row.merge!(resolved: verdict, status: status)
    end
  rescue SignalException
    row.merge!(status: 'interrupted', resolved: nil)
    raise
  rescue => error
    row.merge!(status: 'infrastructure_error', error: error.message)
  ensure
    unless File.directory?("#{slot}/collected")
      command(['docker', 'cp', "#{name}:/opt/willdeep-state", "#{slot}/collected"], "#{slot}/collect-interrupted-state", 60)
    end
    command(['docker', 'rm', '-f', name], "#{slot}/cleanup", 60)
    row[:finished_at] = Time.now.iso8601
    File.write("#{slot}/status.json", JSON.pretty_generate(row))
    summary
  end
  if %w[infrastructure_error evaluation_error interrupted].include?(row[:status])
    File.write("#{ROOT}/progress.json", JSON.pretty_generate({stage: 'blocked', instance_id: id, status: row[:status]}))
    abort 'Infrastructure unavailable; batch paused without skipping remaining tasks.'
  end
  # Keep the first downloaded image for common base layers. Other benchmark
  # images did not exist in the initial Docker inventory; remove only our tag.
  command(['docker', 'image', 'rm', task.fetch('image')], "#{slot}/remove-image", 120) if index.positive?
end
pending = Dir.glob("#{ROOT}/instances/*/status.json").map { |p| JSON.parse(File.read(p)) }.select { |r| %w[evaluation_error infrastructure_error interrupted].include?(r['status']) }
File.write("#{ROOT}/progress.json", JSON.pretty_generate({stage: pending.empty? ? 'complete' : 'blocked', total: 30, pending_ids: pending.map { |r| r['instance_id'] }, reason: pending.empty? ? nil : 'remaining tasks executed; deferred official grading still required'}))
summary
