#!/usr/bin/env ruby
# frozen_string_literal: true

require 'fileutils'
require 'json'
require 'net/http'
require 'open3'
require 'securerandom'
require 'timeout'
require 'tmpdir'
require 'uri'

binary = File.realpath(ARGV.fetch(0))
port = Integer(ARGV.fetch(1), 10)
assert_only = ARGV.include?('--assert-only')
loopback = '127.0.0.1'

Dir.mktmpdir('willdeep-web-workspace-changes-') do |temporary_directory|
  root = File.realpath(temporary_directory)
  fixture_home = File.join(root, 'runtime-home')
  workspace = File.join(root, 'workspace')
  nested = File.join(workspace, 'nested')
  elsewhere = File.join(root, 'elsewhere')
  FileUtils.mkdir_p([fixture_home, nested, elsewhere])
  config = File.join(fixture_home, 'config.toml')
  File.write(config, <<~TOML, perm: 0o600)
    version = 1
    default_provider = "fixture"
    [agent]
    auto_title = false
    input_suggestions = false
    [local_model]
    enabled = false
    [providers.fixture]
    provider = "openai-compatible"
    api = "chat-completions"
    api_base = "http://127.0.0.1:9/v1"
    api_key = "local-fixture-only"
    model = "workspace-diff-fixture"
  TOML
  env = {
    'PATH' => ENV.fetch('PATH'), 'LANG' => 'en_US.UTF-8', 'NO_PROXY' => loopback,
    'WILLDEEP_HOME' => fixture_home, 'WILLDEEP_CONFIG' => config, 'WILLDEEP_LANGUAGE' => 'zh-CN',
    'WILLDEEP_LOCAL_DISCOVERY' => '0', 'WILLDEEP_TELEMETRY_DISABLED' => '1',
    'WILLDEEP_WORKSPACE_CHECKPOINTS' => '0', 'GIT_CONFIG_NOSYSTEM' => '1', 'GIT_CONFIG_GLOBAL' => File::NULL,
    'XDG_CONFIG_HOME' => File.join(fixture_home, '.config'), 'XDG_CACHE_HOME' => File.join(fixture_home, '.cache'),
    'XDG_DATA_HOME' => File.join(fixture_home, '.local/share')
  }
  command = lambda do |*args, input: ''|
    stdout, stderr, status = Open3.capture3(env, *args, stdin_data: input, unsetenv_others: true)
    raise "#{args.first}: #{stderr}" unless status.success?

    stdout
  end
  git = ->(*args) { command.call('git', '-C', workspace, *args) }
  api = lambda do |method, params|
    output = command.call(binary, 'api', method, '--params-file', '-', '--ndjson', input: JSON.generate(params))
    envelope = JSON.parse(output.force_encoding(Encoding::UTF_8))
    raise "#{method}: #{envelope['error']}" unless envelope['status'] == 'ok'

    envelope.fetch('data')
  end
  write = ->(path, body) { File.write(File.join(workspace, path), body) }
  git.call('init', '--quiet')
  write.call('session.txt', "base session line\n")
  write.call('prior.txt', "base prior line\n")
  git.call('add', '.')
  git.call('-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', '-c', 'commit.gpgsign=false', 'commit', '--quiet', '-m', 'Fixture')
  write.call('prior.txt', "unknown prior line\n")

  child = nil
  runtime_started = false
  stopping = false
  %w[INT TERM].each { |signal| Signal.trap(signal) { stopping = true } }
  begin
    command.call(binary, 'daemon', 'start')
    runtime_started = true
    api.call('workspace.register', { root: workspace, name: '工作区差异验证', access: 'smart', provider_profile: 'fixture' })
    api.call('workspace.register', { root: nested, name: '嵌套子目录', access: 'smart', provider_profile: 'fixture' })
    create = ->(title) { api.call('session.create', { workspace: workspace, profile: 'fixture', title: title }).fetch('id') }
    session, other = %w[本会话 另一个会话].map { |title| create.call(title) }
    snapshot = -> { api.call('diff.snapshot', { workspace: workspace }).fetch('id') }
    before = snapshot.call
    write.call('session.txt', "staged session line\n")
    git.call('add', 'session.txt')
    write.call('session.txt', "staged session line\nunstaged session line\n")
    write.call('external.txt', "concurrent external write\n")
    after_session = snapshot.call
    write.call('new file.txt', "other session untracked line\n")
    after_other = snapshot.call

    # Seed file-level provenance with actual Git snapshot IDs. No model or real session is used.
    record = lambda do |id, from, to, paths, confirmed, states|
      { id: SecureRandom.uuid, workspace: workspace, before_snapshot_id: from, after_snapshot_id: to,
        session_id: id, task_id: SecureRandom.uuid, agent_id: SecureRandom.uuid, tool: 'fixture',
        paths: paths, confirmed_paths: confirmed, path_states: states, confidence: 'tool_window', created_at: Time.now.to_i }
    end
    FileUtils.mkdir_p(File.join(fixture_home, 'runtime'))
    File.write(File.join(fixture_home, 'runtime', 'diff-attributions.json'), JSON.generate([
      record.call(session, before, after_session, %w[external.txt session.txt], ['session.txt'], %w[untracked tracked_dirty]),
      record.call(other, after_session, after_other, ['new file.txt'], ['new file.txt'], ['untracked'])
    ]))

    child = Process.spawn(env, binary, '--web', '--listen', "#{loopback}:#{port}", '-c', config, '-w', workspace, '--web-workspace', nested, '--language', 'zh-CN', unsetenv_others: true)
    http = Net::HTTP.new(loopback, port, nil)
    get = ->(path, query = {}) { http.get(query.empty? ? path : "#{path}?#{URI.encode_www_form(query)}") }
    Timeout.timeout(30) do
      loop do
        begin
          break if get.call('/health').is_a?(Net::HTTPSuccess)
        rescue SystemCallError
          raise 'isolated Web exited unexpectedly' if Process.waitpid(child, Process::WNOHANG)
        end
        sleep 0.1
      end
    end
    endpoint = '/api/workspaces/changes'
    selection = { workspace: workspace, session: session }
    changes = get.call(endpoint, selection)
    raise "workspace changes returned #{changes.code}" unless changes.is_a?(Net::HTTPSuccess)
    changes = JSON.parse(changes.body)
    raise 'session attribution includes an unrelated file' unless changes.fetch('session_paths') == ['session.txt']
    raise 'workspace list omits dirty files' unless changes.dig('snapshot', 'files').map { |file| file.fetch('path') }.sort == ['external.txt', 'new file.txt', 'prior.txt', 'session.txt']

    details = "#{endpoint}/file"
    file_query = selection.merge(snapshot_id: after_other)
    [
      ['session.txt', 'staged', '+staged session line', '+unstaged session line'],
      ['session.txt', 'unstaged', '+unstaged session line', '+staged session line'],
      ['new file.txt', 'unstaged', '+other session untracked line', '+staged session line']
    ].each do |path, area, includes, excludes|
      response = get.call(details, file_query.merge(path: path, area: area))
      patch = JSON.parse(response.body)['content'] if response.is_a?(Net::HTTPSuccess)
      raise "incorrect #{area} patch for #{path}: HTTP #{response.code}" unless patch&.include?(includes) && !patch.include?(excludes)
    end
    raise 'parent workspace is visible through a nested root' unless JSON.parse(get.call(endpoint, { workspace: nested }).body)['git'] == false
    raise 'mismatched session workspace accepted' unless get.call(details, file_query.merge(workspace: elsewhere, path: 'session.txt')).code == '403'
    raise 'path outside snapshot accepted' unless get.call(details, file_query.merge(path: '../elsewhere/file.txt')).code == '404'
    write.call('new file.txt', "edited concurrently\n")
    raise 'old snapshot accepted' unless get.call(details, file_query.merge(path: 'new file.txt')).code == '409'
    write.call('new file.txt', "other session untracked line\n")

    puts "Web workspace changes fixture ready: http://#{loopback}:#{port}/?session=#{session}"
    puts 'API checks passed: provenance, staged and unstaged patches, untracked text, nested and mismatched workspaces, missing path, stale snapshot.'
    $stdout.flush
    until stopping || assert_only
      raise 'isolated Web exited unexpectedly' if Process.waitpid(child, Process::WNOHANG)

      sleep 0.2
    end
  ensure
    if child
      begin
        Process.kill('TERM', child)
        Process.waitpid(child)
      rescue Errno::ESRCH, Errno::ECHILD
        # The private child already exited.
      end
    end
    command.call(binary, 'daemon', 'stop') if runtime_started
  end
end
