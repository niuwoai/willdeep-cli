#!/usr/bin/env ruby
require 'json'
require 'net/http'
require 'socket'
require 'tmpdir'
require 'fileutils'
require 'open3'
require 'timeout'
require 'base64'

ROOT = File.expand_path('..', __dir__)
BIN = ENV.fetch('WILLDEEP_BIN', File.join(ROOT, 'target/debug/willdeep'))
PACKAGES = { old: ENV.fetch('WILLDEEP_TODO_OLD_PACKAGE'), fixed: ENV.fetch('WILLDEEP_TODO_FIXED_PACKAGE') }.freeze
IMAGE = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII='.freeze

def command(env, *args)
  out, err, status = Open3.capture3(env, BIN, *args)
  raise "Command failed: #{args.first}: #{err}" unless status.success?
  out
end

def api(url, name, arguments)
  uri = URI("#{url}/api/plugins/willdeep-todo/commands/#{name}")
  response = Net::HTTP.start(uri.host, uri.port, read_timeout: 15) do |http|
    http.post(uri.request_uri, JSON.generate(arguments: arguments), 'content-type' => 'application/json')
  end
  [response.code.to_i, JSON.parse(response.body)]
end

def result(value)
  value = value.fetch('result', value)
  return value if value.key?('ok')
  if value['content']
    text = value.fetch('content').find { |item| item['type'] == 'text' }.fetch('text')
    return JSON.parse(text)
  end
  value
end

checks = []
PACKAGES.each do |label, package|
  Dir.mktmpdir('wd-todo-contract-', '/private/tmp') do |root|
    home = File.join(root, 'home')
    FileUtils.mkdir_p(home, mode: 0o700)
    config = File.join(home, 'config.toml')
    File.write(config, "version=1\ndefault_provider=\"fixture\"\n[local_model]\nenabled=false\n[providers.fixture]\nprovider=\"openai-compatible\"\napi=\"chat-completions\"\napi_base=\"http://127.0.0.1:1/v1\"\napi_key=\"fixture-only\"\nmodel=\"fixture-model\"\n", perm: 0o600)
    store = File.join(root, 'todos.json')
    env = { 'WILLDEEP_HOME' => home, 'WILLDEEP_CONFIG' => config, 'WD_TODO_FILE' => store,
      'WILLDEEP_API_KEY' => nil, 'WILLDEEP_API_BASE' => nil, 'WILLDEEP_MODEL' => nil }
    command(env, 'plugin', 'install', package, '--enable')
    socket = TCPServer.new('127.0.0.1', 0)
    port = socket.addr[1]
    socket.close
    log = File.open(File.join(root, 'web.log'), 'w')
    pid = Process.spawn(env, BIN, '--web', '--listen', "127.0.0.1:#{port}", '-w', root, out: log, err: log)
    begin
      url = "http://127.0.0.1:#{port}"
      Timeout.timeout(20) do
        loop do
          break if (Net::HTTP.get_response(URI("#{url}/health")).code == '200' rescue false)
          sleep 0.1
        end
      end
      if label == :old
        status, body = api(url, 'todo.attachment_preview', id: 'synthetic', attachmentID: 'synthetic')
        raise 'Old mismatch was not reproduced' unless status == 404 && body['error'] == 'unknown contribution'
        checks << 'old 1.4.0 reproduces unknown contribution'
        next
      end
      manifest = JSON.parse(File.read(File.join(package, '.willdeep-plugin/plugin.json')))
      %w[todo.attach todo.attachment_preview todo.detach].each do |name|
        raise "Missing declaration #{name}" unless manifest.dig('contributes', 'commands').any? { |entry| entry['id'] == name }
      end
      status, value = api(url, 'todo.add', text: 'Synthetic attachment contract verification')
      added = result(value)
      raise "Cannot create fixture: #{status} #{added.keys}" unless status == 200 && added['ok']
      id = added.fetch('item').fetch('id')
      status, value = api(url, 'todo.attach', id: id, name: 'contract.png', mimeType: 'image/png', dataBase64: IMAGE)
      attached = result(value)
      raise 'Attach failed' unless status == 200 && attached['ok']
      attachment_id = attached.fetch('attachment').fetch('id')
      status, value = api(url, 'todo.attachment_preview', id: id, attachmentID: attachment_id)
      preview = result(value)
      raise 'Preview bytes changed' unless status == 200 && preview['ok'] && preview['mimeType'] == 'image/png' && Base64.decode64(preview.fetch('dataBase64')) == Base64.decode64(IMAGE)
      checks << 'fixed package attaches and previews exact PNG bytes through HTTP host and MCP'
      status, value = api(url, 'todo.detach', id: id, attachmentID: attachment_id)
      raise 'Detach failed' unless status == 200 && result(value)['ok']
      _, value = api(url, 'todo.attachment_preview', id: id, attachmentID: attachment_id)
      raise 'Removed attachment remained available' unless result(value)['error'] == 'attachment_not_found'
      checks << 'detaching removes fixture attachment; subsequent preview reports missing attachment'
      status, body = api(url, 'todo.not_declared', {})
      raise 'Undeclared command was not rejected' unless status == 404 && body['error'] == 'unknown contribution'
      checks << 'undeclared commands remain rejected'
    ensure
      Process.kill('TERM', pid)
      Process.wait(pid)
      log.close
      Open3.capture3(env, BIN, 'daemon', 'stop')
    end
  end
end
directory = File.join(ROOT, 'build/verification')
FileUtils.mkdir_p(directory)
File.write(File.join(directory, 'todo-plugin-contract.json'), JSON.pretty_generate(passed: true, checks: checks, isolated_store: true, external_model_requests: 0))
File.write(File.join(directory, 'todo-plugin-contract.md'), "# 待办插件命令契约验证\n\n" + checks.map { |check| "- PASS: #{check}\n" }.join)
puts JSON.generate(passed: true, checks: checks)
