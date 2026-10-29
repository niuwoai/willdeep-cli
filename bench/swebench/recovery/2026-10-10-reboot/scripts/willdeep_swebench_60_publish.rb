require 'json'
require 'digest'
require 'fileutils'
require 'zlib'
require 'rubygems/package'
require 'net/http'
require 'openssl'
require 'base64'
require 'time'
require '/Users/rocky/Sites/new83d/scripts/lib/q3_release'

root = File.expand_path(ARGV.fetch(0))
raise 'Unexpected run root' unless root.start_with?('/private/tmp/willdeep-swebench-094rc3-')
meta = JSON.parse(File.read("#{root}/manifest.json"))
report = JSON.parse(File.read("#{root}/summary.json"))
slug = meta.fetch('run_id')
expected = meta.fetch('instance_ids')
rows = report.fetch('instances')
raise 'Incomplete fixed 60-task run' unless expected.size == 60 && rows.map { |r| r['instance_id'] }.sort == expected.sort && rows.all? { |r| r.key?('agent_exit') && [true, false].include?(r['resolved']) && !%w[evaluation_error infrastructure_error interrupted].include?(r['status']) }
raise 'Resolved count inconsistent with terminal rows' unless report.fetch('resolved') == rows.count { |r| r['resolved'] == true }
verify_old = lambda do
  JSON.parse(File.read("#{root}/previous-report-sha256.json")).each do |path, sha|
    raise "Previous report changed: #{path}" unless File.file?(path) && Digest::SHA256.file(path).hexdigest == sha
  end
end
verify_old.call
stage = "#{root}/publication"
folder = "#{stage}/public/benchmarks/runs/#{slug}"
raise 'Already exported; use saved publication result, do not overwrite' if File.exist?("#{root}/publication-result.json")
FileUtils.mkdir_p(folder, mode: 0o700)
secrets = File.readlines("#{root}/config.toml").filter_map { |line| m = line.match(/^\s*[\w-]*(?:key|token|secret|password)[\w-]*\s*=\s*"([^"]+)"/i); m && m[1].size >= 8 ? m[1] : nil }.uniq
redactions = Hash.new(0)
scrub = lambda do |value|
  case value
  when String
    text = value.encode('UTF-8', invalid: :replace, undef: :replace)
    secrets.each { |key| redactions['known_credential'] += text.scan(Regexp.new(Regexp.escape(key))).size; text = text.gsub(key, '[REDACTED_CREDENTIAL]') }
    patterns = {token_pattern: /\b(?:sk-[A-Za-z0-9_-]{16,}|LTAI[A-Za-z0-9]{12,}|gh[pousr]_[A-Za-z0-9]{20,})\b/,
      authorization: /(?:Bearer|Basic)\s+[A-Za-z0-9+\/=_.-]{12,}/i,
      credential_assignment: /(?:api[_-]?key|access[_-]?key[_-]?secret|password|secret[_-]?key|auth[_-]?token)\s*[=:]\s*["']?[^\s"',;\}\]]{8,}/i,
      local_home: %r{/Users/[^/\s"']+}, email: /\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b/}
    patterns.each { |kind, pattern| redactions[kind] += text.scan(pattern).size; text = text.gsub(pattern, "[REDACTED_#{kind.to_s.upcase}]") }
    text
  when Array then value.map { |item| scrub.call(item) }
  when Hash then value.to_h { |key, item| [key, scrub.call(item)] }
  else value
  end
end
records = (Dir.glob("#{root}/instances/*/collected/usage/*.jsonl") + Dir.glob("#{root}/instances/*/budget-usage.jsonl")).flat_map { |p| File.readlines(p).filter_map { |l| JSON.parse(l) rescue nil } }.uniq { |r| r.fetch('id') }
usage = {model_calls: records.size, models: records.group_by { |r| r['model'] }.transform_values(&:size), kinds: records.group_by { |r| r['kind'] }.transform_values { |rs| {calls: rs.size, reported_total_tokens: rs.sum { |r| r['total_tokens'].to_i }} }}
%w[input_tokens output_tokens cache_read_tokens total_tokens].each { |key| values = records.filter_map { |r| r[key] }; usage[key] = values.empty? ? nil : values.sum; usage["#{key}_known_calls"] = values.size }
report['usage'] = usage
predictions = []
public_rows = expected.map do |id|
  row = rows.find { |r| r['instance_id'] == id }
  private_task = "#{root}/instances/#{id}"
  dest = "#{folder}/tasks/#{id}"
  FileUtils.mkdir_p(dest)
  prediction = JSON.parse(File.read("#{private_task}/prediction.jsonl"))
  patch = prediction.fetch('model_patch')
  raise 'Patch credential detected; publication blocked, exact prediction preserved privately' if secrets.any? { |key| patch.include?(key) } || patch.match?(/\b(?:sk-[A-Za-z0-9_-]{16,}|LTAI[A-Za-z0-9]{12,}|gh[pousr]_[A-Za-z0-9]{20,})\b/)
  raise 'Wrong prediction identity' unless prediction['instance_id'] == id && prediction['model_name_or_path'] == "willdeep-#{meta['binary_version']}-#{meta['model']}"
  predictions << prediction
  sessions = Dir.glob("#{private_task}/collected/sessions/*.json").sort.map do |path|
    raw = JSON.parse(File.read(path))
    scrub.call({id: raw['id'], model: raw['model'], created_at: raw['created_at'], updated_at: raw['updated_at'], messages: raw.fetch('messages').map { |m| m.slice('role', 'source', 'content', 'reasoning', 'tool_calls', 'tool_call_id') }})
  end
  outputs = Dir.glob("#{private_task}/collected/tool-outputs/**/*").select { |p| File.file?(p) }.map { |p| {id: p.split('/tool-outputs/').last, content: scrub.call(File.read(p))} }
  trace = {instance_id: id, root_prompt: scrub.call(File.read("#{private_task}/prompt.md")), sessions: sessions, tool_outputs: outputs, coverage: 'Persisted main/worker messages and archived tool outputs; not provider wire logs. Compression and in-flight output may be missing.'}
  official_path = "#{root}/logs/evaluation/#{slug}-#{id}/willdeep-#{meta['binary_version']}-#{meta['model']}/#{id}/report.json"
  official = File.file?(official_path) ? JSON.parse(File.read(official_path)) : nil
  raise 'Nonempty patch has no matching official verdict' unless patch.empty? || official && official.fetch(id).fetch('resolved') == row['resolved']
  File.write("#{dest}/patch.diff", patch)
  File.write("#{dest}/trace.json", JSON.generate(trace))
  File.write("#{dest}/evidence.json", JSON.generate({status: scrub.call(row), official_report: official, patch: patch, patch_sha256: Digest::SHA256.hexdigest(patch), trace: trace}))
  task_records = (Dir.glob("#{private_task}/collected/usage/*.jsonl") + Dir.glob("#{private_task}/budget-usage.jsonl")).flat_map { |p| File.readlines(p).filter_map { |l| JSON.parse(l) rescue nil } }.uniq { |r| r['id'] }
  {id: id, status: row['status'], resolved: row['resolved'], seconds: row['agent_seconds'], tokens: task_records.sum { |r| r['total_tokens'].to_i }, sessions: sessions.size, messages: sessions.sum { |s| s[:messages].size }, evidence: "/benchmarks/runs/#{slug}/tasks/#{id}/evidence.json"}
end
File.write("#{folder}/predictions.jsonl", predictions.map { |p| JSON.generate(p) }.join("\n") + "\n")
report['completed'] = 60
File.write("#{folder}/summary.json", JSON.pretty_generate(scrub.call(report)))
File.write("#{folder}/manifest.json", JSON.pretty_generate(scrub.call(meta.reject { |key, _| key == 'binary_path' })))
seconds = rows.sum { |r| r['agent_seconds'].to_f }
markdown = "# WillDeep CLI #{meta['binary_version']} · #{meta['model']} · 固定60题\n\n有效完成60/60，官方解决 #{report['resolved']}/60（#{(report['resolved'] / 60.0 * 100).round(1)}%）。\n\n状态：#{report['statuses'].map { |k,v| "#{k}: #{v}" }.join('；')}。\n\n已报告模型调用 #{records.size} 次，共 #{usage[:total_tokens] || '未知'} token；主/子/辅助统计及模型分布见 summary.json。Agent累计耗时 #{seconds.round(1)} 秒；费用未知。预算、超额、超时逐题保留。\n\n这是一套覆盖12项目的探索性60题子集，不是完整Verified500题成绩；使用CLI run --local，Mac版未单独评测。\n\n|题目|状态|官方解决|Agent秒数|\n|---|---|---|---|\n" + rows.map { |r| "|#{r['instance_id']}|#{r['status']}|#{r['resolved']}|#{r['agent_seconds']}|" }.join("\n") + "\n"
File.write("#{folder}/summary.md", markdown)
legacy_root = meta['model'] == 'glm-5' ? '/Users/rocky/Sites/willdeep-rs/bench/swebench/runs/2026-10-09-glm5-30' : '/Users/rocky/Sites/willdeep-rs/bench/swebench/runs/2026-10-08-default-30'
legacy = JSON.parse(File.read("#{legacy_root}/summary.json"))
old_rows = legacy.fetch('instances')
paired = old_rows.map { |old| [old, rows.find { |row| row['instance_id'] == old['instance_id'] }] }
raise 'Historical 30-task pairing incomplete' unless paired.size == 30 && paired.all? { |_, current| current }
retained = paired.count { |old, current| old['resolved'] == true && current['resolved'] == true }
gained = paired.count { |old, current| old['resolved'] == false && current['resolved'] == true }
lost = paired.count { |old, current| old['resolved'] == true && current['resolved'] == false }
comparison = "# 历史30题配对复测\n\n同一根模型 #{meta['model']}：CLI 0.92.0-rc1 的历史30题解决 #{old_rows.count { |r| r['resolved'] == true }}/30；CLI #{meta['binary_version']} 在这30题上解决 #{paired.count { |_, r| r['resolved'] == true }}/30。保持解决 #{retained}，新增解决 #{gained}，退步 #{lost}。本轮完整60题分数单列，不能直接与30题百分比当作同题集提升。提示词与运行时同时变化，结果不能归因于单一改动。每题预算相同，模型与辅助模型实际调用分布见报告；单次运行有随机性。\n\n|题目|旧版30题判定|本版配对判定|\n|---|---|---|\n" + paired.map { |old,current| "|#{old['instance_id']}|#{old['resolved']}|#{current['resolved']}|" }.join("\n") + "\n"
File.write("#{folder}/comparison.md", comparison)
File.write("#{folder}/README.md", "# #{slug}\n\n同一份冻结 verified-60-v1 清单与顺序，WillDeep CLI #{meta['binary_version']}，#{meta['model']} 根模型。每题60轮、1200秒、全部已报告主子辅助调用累计100万token阈值；末次及在途调用可超额，实际用量见报告。官方裁判独立容器1200秒。主容器2核3GB，linux/amd64在ARM64仿真，整批并发1。\n\n脱敏持久化会话、工具输入输出、原始问题、精确提交补丁及官方结果均可下载。未包含凭据、配置、遥测、工作树缓存、gold/test参考补丁或Provider HTTP头。原始提交物不修改；发现秘密会阻止发布。历史30题报告保持不变。\n\n任务清单SHA256：#{meta['taskset_sha256']}\nCLI二进制SHA256：#{meta['linux_binary_sha256']}\n官方评测器commit：#{meta['official_commit']}\n任务定义commit：#{meta['task_repo_commit']}\n")
File.write("#{folder}/completion-check.json", JSON.pretty_generate({fixed_ids_verified: true, all_models_executed: true, valid_terminal_count: 60, official_verdicts_verified: true, previous_reports_unchanged: true}))
File.write("#{folder}/publication-audit.json", JSON.pretty_generate({policy_version: 'public-trace-v1', included: %w[prompts session_messages tool_calls tool_outputs exact_submitted_patches official_reports], excluded: %w[config credentials telemetry checkpoints provider_http_headers reference_patches], redactions: redactions, limitations: 'Persisted traces, not complete provider HTTP wire logs; no model reruns.'}))
files = Dir.glob("#{folder}/**/*").select { |p| File.file?(p) }
files.each { |p| raise 'Credential in export' if secrets.any? { |key| File.read(p).include?(key) } }
File.write("#{folder}/checksums.json", JSON.pretty_generate(files.sort.to_h { |p| [p.delete_prefix(folder + '/'), Digest::SHA256.file(p).hexdigest] }))
archive = "#{stage}/evidence.tar.gz"
Zlib::GzipWriter.open(archive) { |gz| Gem::Package::TarWriter.new(gz) { |tar| Dir.glob("#{folder}/**/*").select { |p| File.file?(p) }.sort.each { |p| bytes = File.binread(p); tar.add_file_simple(p.delete_prefix(folder + '/'), 0o644, bytes.bytesize) { |io| io.write(bytes) } } } }
sha = Digest::SHA256.file(archive).hexdigest
key = "willdeep/benchmarks/#{slug}/#{sha}/evidence.tar.gz"
url = "https://img.niuwoai.com/#{key}"
credentials = Q3Release.oss_credentials.transform_keys(&:to_s)
payload = credentials.merge('bucket' => 'sz-trans', 'keys' => [key], 'sha256' => sha)
Q3Release.validate_oss_payload!(payload)
# Public ACL is limited to this immutable, audited evidence object.
endpoint = payload.fetch('endpoint').sub(%r{\Ahttps?://}, '').sub(%r{/+\z}, '')
date = Time.now.httpdate
type = 'application/gzip'
canonical = ["PUT", "", type, date, "x-oss-object-acl:public-read\n/sz-trans/#{key}"].join("\n")
signature = Base64.strict_encode64(OpenSSL::HMAC.digest('sha1', payload.fetch('access_key_secret'), canonical))
uri = URI("https://sz-trans.#{endpoint}/#{key}")
request = Net::HTTP::Put.new(uri)
request['Date'] = date; request['Content-Type'] = type; request['x-oss-object-acl'] = 'public-read'
request['Authorization'] = "OSS #{payload.fetch('access_key_id')}:#{signature}"
request.content_length = File.size(archive)
File.open(archive, 'rb') { |file| request.body_stream = file; response = Net::HTTP.start(uri.host, uri.port, use_ssl: true, open_timeout: 30, read_timeout: 300) { |http| http.request(request) }; raise "OSS upload failed HTTP #{response.code}" unless response.is_a?(Net::HTTPSuccess) }
response = Net::HTTP.get_response(URI(url))
raise 'Public archive readback checksum failed' unless response.is_a?(Net::HTTPSuccess) && Digest::SHA256.hexdigest(response.body) == sha
verify_old.call
run = {id: slug, model: meta['model'], taskset: meta['taskset'], taskset_sha256: meta['taskset_sha256'], planned: 60, completed: 60, state: 'complete', resolved: report['resolved'], statuses: report['statuses'], binary_version: meta['binary_version'], product: 'cli', platform: 'linux/amd64 (ARM64 emulation)', execution_mode: 'run --local', usage: usage, started_at: rows.map { |r| r['started_at'] }.min, finished_at: rows.map { |r| r['finished_at'] }.max, rows: public_rows, limits: {max_turns: 60, agent_timeout_seconds: 1200, all_agent_token_threshold: 1_000_000}, archive: {url: url, sha256: sha, bytes: File.size(archive)}, report_url: "/benchmarks/runs/#{slug}/summary.md", predictions_url: "/benchmarks/runs/#{slug}/predictions.jsonl", audit_url: "/benchmarks/runs/#{slug}/publication-audit.json"}
File.write("#{root}/publication-result.json", JSON.pretty_generate({folder: folder, run: run}), perm: 0o600)
out = "/Users/rocky/Sites/willdeep-rs/bench/swebench/runs/#{slug}"
raise 'Public report destination exists; never overwrite' if File.exist?(out)
FileUtils.mkdir_p(out)
Dir.glob("#{folder}/*").each { |file| FileUtils.cp_r(file, out) }
puts JSON.generate({id: slug, resolved: report['resolved'], archive: url, sha256: sha, public_report: out})
