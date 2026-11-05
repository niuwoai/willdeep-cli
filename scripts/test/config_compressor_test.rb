#!/usr/bin/env ruby
# frozen_string_literal: true

# 配置服务语义回归；仅合成配置和临时目录，报告可重复生成。
require 'json'
require 'tmpdir'
require 'fileutils'
require 'timeout'
require 'open3'
require 'rbconfig'
require_relative '../../crates/willdeep-cli/assets/willdeep-config/server/config_server'

checks = []
check = lambda do |name, &block|
  block.call
  checks << { name: name, status: 'passed' }
rescue StandardError => e
  checks << { name: name, status: 'failed', error: e.message }
end
assert = ->(value, message) { raise message unless value }
edit = ->(key, value) { { 'op' => 'set', 'section' => 'agent', 'key' => key, 'value' => value } }
sample = <<~TOML
  version = 1
  [providers.summary]
  provider = "openai-compatible"
  model = "fixture-summary"
  context_window = 65536
TOML

Dir.mktmpdir('wd-compressor-config-') do |home|
  path = File.join(home, 'selected.toml')
  backup = File.join(home, 'backups')
  box = WilldeepConfig::Toolbox.new(config_path: path, backup_dir: backup)
  File.write(path, sample)
  edits = [edit.call('compressor_model', 'fixture-override'),
           edit.call('compressor_profile', 'summary'),
           edit.call('compressor_context_window', 32768)]

  check.call('plan previews all three compressor fields without writing') do
    plan = box.call('config.plan', { 'edits' => edits })
    assert.call(plan['ok'], plan.inspect)
    edits.each { |entry| assert.call(plan['diff'].include?(entry['key']), 'missing preview field') }
    assert.call(File.read(path) == sample, 'plan wrote the config')
    assert.call(!File.exist?(backup), 'plan created backups')
  end

  check.call('apply requires confirmation') do
    result = box.call('config.apply', { 'edits' => edits })
    assert.call(result['ok'] == false, 'unconfirmed apply succeeded')
    assert.call(File.read(path) == sample, 'unconfirmed apply wrote config')
  end

  check.call('apply saves three fields with an exact backup') do
    result = box.call('config.apply', { 'edits' => edits, 'confirm' => true })
    assert.call(result['ok'], result.inspect)
    assert.call(File.read(result.fetch('backupPath')) == sample, 'backup mismatch')
    root = TomlLite.parse(File.read(path))
    edits.each { |entry| assert.call(root['agent'][entry['key']] == entry['value'], 'field mismatch') }
    assert.call(File.stat(path).mode & 0o777 == 0o600, 'config is not private') unless Gem.win_platform?
  end

  [3999, 1_048_577, -1, 0, 1.5, '32768'].each do |window|
    check.call("invalid compressor window #{window.inspect} is refused") do
      before = File.read(path)
      args = { 'edits' => [edit.call('compressor_context_window', window)], 'confirm' => true }
      %w[config.plan config.apply].each do |tool|
        assert.call(box.call(tool, args)['ok'] == false, "#{tool} accepted an invalid window")
      end
      assert.call(File.read(path) == before, 'invalid window was written')
    end
  end

  ['', '  ', 'missing'].each do |profile|
    check.call("invalid compressor profile #{profile.inspect} is refused") do
      before = File.read(path)
      args = { 'edits' => [edit.call('compressor_profile', profile)], 'confirm' => true }
      %w[config.plan config.apply].each do |tool|
        assert.call(box.call(tool, args)['ok'] == false, "#{tool} accepted an invalid profile")
      end
      assert.call(File.read(path) == before, 'invalid profile was written')
    end
  end

  check.call('deleting a referenced provider is refused') do
    before = File.read(path)
    args = { 'edits' => [{ 'op' => 'removeSection', 'section' => 'providers.summary' }], 'confirm' => true }
    %w[config.plan config.apply].each do |tool|
      assert.call(box.call(tool, args)['ok'] == false, "#{tool} removed a referenced provider")
    end
    assert.call(File.read(path) == before, 'referenced provider was deleted')
  end

  check.call('creating and referencing a provider in the same batch is valid') do
    args = { 'confirm' => true, 'edits' => [edit.call('compressor_profile', 'new-summary'),
      { 'op' => 'set', 'section' => 'providers.new-summary', 'key' => 'model', 'value' => 'fixture-new' }] }
    assert.call(box.call('config.apply', args)['ok'], 'same-batch provider was refused')
  end

  check.call('clearing the reference and removing its provider is valid') do
    args = { 'confirm' => true, 'edits' => [
      { 'op' => 'unset', 'section' => 'agent', 'key' => 'compressor_profile' },
      { 'op' => 'removeSection', 'section' => 'providers.new-summary' }] }
    assert.call(box.call('config.apply', args)['ok'], 'same-batch reference removal was refused')
  end

  check.call('concurrent saves with the same revision have one winner') do
    concurrent = File.join(home, 'concurrent.toml')
    File.write(concurrent, sample)
    local = WilldeepConfig::Toolbox.new(config_path: concurrent, backup_dir: File.join(home, 'concurrent-backups'))
    args = { 'edits' => [edit.call('compressor_context_window', 32768)] }
    revision = local.call('config.plan', args).fetch('revision')
    results = [32768, 65536].map do |window|
      Thread.new do
        service = WilldeepConfig::Toolbox.new(config_path: concurrent, backup_dir: File.join(home, 'concurrent-backups'))
        service.call('config.apply', { 'edits' => [edit.call('compressor_context_window', window)],
          'confirm' => true, 'expectedRevision' => revision })
      end
    end.map(&:value)
    assert.call(results.count { |result| result['ok'] } == 1, 'concurrent writes both committed')
    assert.call(results.count { |result| result['error'] == 'conflict' } == 1, 'stale revision was not rejected')
    assert.call(Dir.glob(File.join(home, 'concurrent-backups', '*.bak')).size == 1, 'conflicting save created a backup')
  end

  check.call('same-basename targets share a backup lock without overwriting originals') do
    sources = %w[first second].map do |name|
      directory = File.join(home, name)
      FileUtils.mkdir_p(directory)
      source = File.join(directory, 'config.toml')
      File.write(source, sample + "\n# #{name}\n")
      source
    end
    shared = File.join(home, 'shared-backups')
    FileUtils.mkdir_p(shared)
    ready = Queue.new
    done = Queue.new
    threads = []
    premature = false
    File.open(File.join(shared, '.backup.lock'), File::RDWR | File::CREAT, 0o600) do |lock|
      lock.flock(File::LOCK_EX)
      threads = sources.map do |source|
        Thread.new do
          ready << true
          service = WilldeepConfig::Toolbox.new(config_path: source, backup_dir: shared)
          result = service.call('config.apply', { 'edits' => [edit.call('compressor_context_window', 32768)], 'confirm' => true })
          done << result
          result
        end
      end
      2.times { ready.pop }
      begin
        Timeout.timeout(0.1) { done.pop; premature = true }
      rescue Timeout::Error
        # Both saves must wait for the backup directory's interprocess lock.
      ensure
        lock.flock(File::LOCK_UN)
      end
    end
    results = threads.map(&:value)
    assert.call(!premature, 'backup publication ignored the shared lock')
    assert.call(results.all? { |result| result['ok'] }, 'save failed')
    backups = results.map { |result| result.fetch('backupPath') }
    assert.call(backups.uniq.size == 2, 'different sources overwrote one backup')
    originals = backups.map { |backup| File.read(backup) }.sort
    assert.call(originals == %w[first second].map { |name| sample + "\n# #{name}\n" }.sort, 'backup content mismatch')
  end

  check.call('private stdio supports a Unicode config path under the C locale') do
    unicode_path = File.join(home, '自定义配置.toml')
    File.write(unicode_path, sample)
    request = { jsonrpc: '2.0', id: 1, method: 'tools/call', params: {
      name: 'config.plan', arguments: { edits: [edit.call('compressor_context_window', 32768)] } } }
    environment = { 'HOME' => home, 'PATH' => ENV.fetch('PATH'), 'LC_ALL' => 'C',
      'WD_CONFIG_FILE' => unicode_path, 'WD_HOME' => home, 'WD_CONFIG_BACKUP_DIR' => backup }
    output, _error, status = Open3.capture3(environment, RbConfig.ruby,
      File.expand_path('../../crates/willdeep-cli/assets/willdeep-config/server/config_server.rb', __dir__),
      stdin_data: JSON.generate(request) + "\n", unsetenv_others: true)
    assert.call(status.success?, 'private stdio failed')
    response = JSON.parse(JSON.parse(output).dig('result', 'content', 0, 'text'))
    assert.call(response['ok'], 'Unicode preview was refused')
    assert.call(response['path'] == unicode_path, 'Unicode target changed')
  end

  check.call('validate catches compressor dependency and range errors') do
    bad = sample + "\n[agent]\ncompressor_profile = \"absent\"\ncompressor_context_window = 3999\n"
    result = box.call('config.validate', { 'text' => bad })
    assert.call(result['valid'] == false, 'invalid config was validated')
    assert.call(result['errors'].size == 2, 'missing semantic errors')
  end
end

report = { suite: 'config-compressor', checks: checks,
           passed: checks.count { |item| item[:status] == 'passed' },
           failed: checks.count { |item| item[:status] == 'failed' } }
if ARGV[0]
  FileUtils.mkdir_p(ARGV[0])
  File.write(File.join(ARGV[0], 'compressor.json'), JSON.pretty_generate(report) + "\n")
  lines = ["# 压缩配置服务验收", '', "通过 #{report[:passed]}，失败 #{report[:failed]}。", '', '| 用例 | 结果 |', '|---|---|']
  checks.each { |item| lines << "| #{item[:name]} | #{item[:status]}#{item[:error] ? ': ' + item[:error] : ''} |" }
  File.write(File.join(ARGV[0], 'compressor.md'), lines.join("\n") + "\n")
end
puts JSON.pretty_generate(report)
exit(report[:failed].zero? ? 0 : 1)
