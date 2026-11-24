#!/usr/bin/env ruby
# frozen_string_literal: true

# Archive a Harbor Terminal-Bench job directory into bench/terminal-bench/.
#
# Usage:
#   ruby scripts/archive_terminal_bench_job.rb jobs/<job-name>
#
# Appends one line to bench/terminal-bench/history.jsonl and writes
# reports/<date>/<model>.<hhmmss>.json + .md (model sanitized from result eval key).

require "json"
require "fileutils"
require "time"
require "open3"

ROOT = File.expand_path("..", __dir__)
BENCH = File.join(ROOT, "bench", "terminal-bench")
REPORTS = File.join(BENCH, "reports")

job_dir = ARGV[0]
if job_dir.nil? || job_dir.empty?
  warn "usage: ruby scripts/archive_terminal_bench_job.rb jobs/<job-name>"
  exit 1
end

job_dir = File.expand_path(job_dir, ROOT)
result_path = File.join(job_dir, "result.json")
unless File.file?(result_path)
  warn "missing #{result_path}"
  exit 1
end

result = JSON.parse(File.read(result_path))
job_name = File.basename(job_dir)
finished = result["finished_at"] || result["updated_at"] || Time.now.utc.iso8601
t = Time.parse(finished).utc
stamp_date = t.strftime("%Y-%m-%d")
stamp_clock = t.strftime("%H%M%S")

eval_key = (result.dig("stats", "evals")&.keys || ["unknown"]).first
model = eval_key.split("__")[1] || "unknown"
safe_model = model.gsub(/[^a-zA-Z0-9._-]/, "-")

commit, = Open3.capture2("git", "-C", ROOT, "rev-parse", "--short", "HEAD")
commit = commit.strip
_dirty, = Open3.capture2("git", "-C", ROOT, "status", "--porcelain")
dirty = !_dirty.strip.empty?

stats = result["stats"] || {}
exception_stats = {}
(result.dig("stats", "evals", eval_key, "exception_stats") || {}).each do |exc, ids|
  exception_stats[exc] = ids.size
end

n_passed = (result.dig("stats", "evals", eval_key, "n_trials") || 0).to_i
n_errors = (result.dig("stats", "evals", eval_key, "n_errors") || stats["n_errored_trials"] || 0).to_i

report_base = File.join(REPORTS, stamp_date, "#{safe_model}.#{stamp_clock}")
FileUtils.mkdir_p(File.dirname(report_base))

payload = {
  "schema" => "willdeep/terminal-bench-report/v1",
  "archived_at" => Time.now.utc.iso8601,
  "job_name" => job_name,
  "harbor_result" => result,
  "git" => { "commit" => commit, "dirty" => dirty }
}

json_out = "#{report_base}.json"
File.write(json_out, JSON.pretty_generate(payload))

md_out = "#{report_base}.md"
lines = [
  "# Terminal-Bench archive: #{job_name}",
  "",
  "| Field | Value |",
  "|---|---|",
  "| Finished (UTC) | #{finished} |",
  "| Eval | #{eval_key} |",
  "| Trials total | #{result['n_total_trials']} |",
  "| Errored | #{stats['n_errored_trials']} |",
  "| Eval trials (passed) | #{n_passed} |",
  "| Commit | `#{commit}`#{dirty ? ' (dirty)' : ''} |",
  "",
  "## Exceptions",
  ""
]
if exception_stats.empty?
  lines << "_(none recorded)_"
else
  exception_stats.each { |k, v| lines << "- **#{k}**: #{v}" }
end
lines += ["", "Full JSON: `#{json_out.sub(ROOT + '/', '')}`"]
File.write(md_out, lines.join("\n") + "\n")

history_line = {
  "ran_at" => t.iso8601,
  "job_name" => job_name,
  "model" => model,
  "dataset" => "terminal-bench",
  "commit" => commit,
  "dirty" => dirty,
  "tasks_scheduled" => result["n_total_trials"],
  "n_passed" => n_passed,
  "n_infra_errors" => n_errors,
  "pass_rate" => (result["n_total_trials"].to_i.positive? && n_passed.positive? ? (n_passed.to_f / result["n_total_trials"]) : nil),
  "exception_stats" => exception_stats,
  "report" => json_out.sub(ROOT + "/", "")
}
File.open(File.join(BENCH, "history.jsonl"), "a") { |f| f.puts JSON.generate(history_line) }

puts "Wrote #{json_out}"
puts "Wrote #{md_out}"
puts "Appended bench/terminal-bench/history.jsonl"
