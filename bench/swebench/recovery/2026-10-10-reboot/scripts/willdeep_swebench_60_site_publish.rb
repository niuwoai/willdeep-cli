require 'json'
require 'digest'
require 'fileutils'
require 'open3'
require 'time'
SITE = '/Users/rocky/Sites/willdeep-react-static'
BATCH = '/private/tmp/willdeep-swebench-094rc3-batch'
ROOTS = %w[deepseek-flash glm5].map { |name| "/private/tmp/willdeep-swebench-094rc3-#{name}-60-20261009-v1" }
def run(*argv, dir: SITE)
  output, error, status = Open3.capture3(*argv, chdir: dir)
  raise "Command failed: #{argv.first(2).join(' ')}: #{error.lines.last.to_s.strip}" unless status.success?
  output
end
raise 'Website result publication already recorded; verify saved commit instead of republishing' if File.file?("#{BATCH}/site-publication.json")
publications = ROOTS.map { |root| JSON.parse(File.read("#{root}/publication-result.json")) }
ROOTS.each do |root|
  JSON.parse(File.read("#{root}/previous-report-sha256.json")).each { |path, sha| raise 'Previous report changed' unless File.file?(path) && Digest::SHA256.file(path).hexdigest == sha }
end
run('git', 'fetch', 'origin')
stamp = Time.now.utc.strftime('%Y%m%d%H%M%S')
branch = "codex/benchmark-results-094rc3-#{stamp}"
checkout = "/private/tmp/willdeep-site-results-094rc3-#{stamp}"
run('git', 'worktree', 'add', '-b', branch, checkout, 'origin/main')
index_path = "#{checkout}/public/benchmarks/index.json"
index = JSON.parse(File.read(index_path))
previous_runs = index.fetch('runs').reject { |r| publications.any? { |p| p.fetch('run').fetch('id') == r['id'] } }
publications.each do |publication|
  candidate = publication.fetch('run')
  raise 'Expected complete 60-task result' unless candidate['state'] == 'complete' && candidate['completed'] == 60 && candidate['rows'].size == 60
  raise 'Frozen taskset mismatch' unless candidate['taskset_sha256'] == index.fetch('taskset').fetch('sha256')
  position = index.fetch('runs').index { |r| r['id'] == candidate['id'] }
  raise 'Prepared run registration missing' unless position
  raise 'Complete run already published' if index['runs'][position]['state'] == 'complete'
  index['runs'][position] = candidate
  dest = "#{checkout}/public/benchmarks/runs/#{candidate['id']}"
  FileUtils.mkdir_p(dest)
  Dir.glob("#{publication.fetch('folder')}/*").each { |path| FileUtils.cp_r(path, dest) }
end
raise 'Historical run objects changed' unless previous_runs.all? { |old| index['runs'].find { |r| r['id'] == old['id'] } == old }
File.write(index_path, JSON.pretty_generate(index))
File.write("#{checkout}/src/data/benchmarks.json", JSON.generate(index))
package_path = "#{checkout}/package.json"
package = JSON.parse(File.read(package_path))
old_version = package.fetch('version')
match = old_version.match(/\A(\d+\.\d+\.\d+)-rc(\d+)\z/)
raise 'Expected rc version; reconcile site version manually' unless match
version = "#{match[1]}-rc#{match[2].to_i + 1}"
package['version'] = version
File.write(package_path, JSON.pretty_generate(package) + "\n")
%w[nginx.conf.template PRODUCT_OVERVIEW.md].each { |path| file = "#{checkout}/#{path}"; text = File.read(file).gsub(old_version, version).sub('新增30题尚未评测。', 'CLI 0.94.0-rc3 的 DeepSeek、GLM-5 两轮固定60题均已完成核验并独立发布。'); File.write(file, text) }
changelog = "#{checkout}/CHANGELOG.md"
text = File.read(changelog).sub(/^# Changelog\s*\n/, "# Changelog\n\n## [#{version}] - #{Time.now.strftime('%Y-%m-%d')}\n\n### Changed\n- 发布 CLI 0.94.0-rc3 的 DeepSeek V4 Flash、GLM-5 两轮固定60题完整成绩、脱敏证据与独立OSS下载；保留所有历史批次。\n\n")
File.write(changelog, text)
run('yarn', 'install', '--frozen-lockfile', dir: checkout)
run('yarn', 'test', dir: checkout)
run('yarn', 'build', dir: checkout)
run('git', 'diff', '--check', dir: checkout)
paths = %w[package.json nginx.conf.template PRODUCT_OVERVIEW.md CHANGELOG.md public/benchmarks/index.json src/data/benchmarks.json] + publications.map { |p| "public/benchmarks/runs/#{p.fetch('run').fetch('id')}" }
run('git', 'add', *paths, dir: checkout)
run('git', 'commit', '-m', "docs(benchmarks): publish CLI 0.94.0-rc3 fixed-60 results", dir: checkout)
run('git', 'fetch', 'origin', dir: checkout)
release = "/private/tmp/willdeep-site-results-release-#{stamp}"
run('git', 'worktree', 'add', '--detach', release, 'origin/main')
run('git', 'merge', '--no-ff', branch, '-m', "Merge verified benchmark results for v#{version}", dir: release)
run('git', 'tag', '-a', "v#{version}", '-m', "v#{version}", dir: release)
run('git', 'push', '--atomic', 'origin', 'HEAD:main', "v#{version}", dir: release)
commit = run('git', 'rev-parse', 'HEAD', dir: release).strip
File.write("#{BATCH}/site-publication.json", JSON.pretty_generate({version: version, commit: commit, worktree: checkout, release_worktree: release, runs: publications.map { |p| p.fetch('run').fetch('id') }, at: Time.now.iso8601}), perm: 0o600)
puts JSON.generate({version: version, commit: commit, site: 'https://willdeep.com/benchmarks/'})
