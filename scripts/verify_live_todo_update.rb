#!/usr/bin/env ruby
# Read-only checks against the real service; never creates or changes a todo.
require 'net/http'
require 'json'
require 'digest'
require 'fileutils'
require 'base64'

base = URI(ENV.fetch('WILLDEEP_WEB_URL', 'http://127.0.0.1:9847'))
metadata = JSON.parse(File.read(ENV.fetch('WILLDEEP_UPDATE_METADATA')))
expected = ENV.fetch('WILLDEEP_EXPECTED_VERSION')
get = lambda do |path|
  response = Net::HTTP.start(base.host, base.port, read_timeout: 15) { |http| http.get(path) }
  raise "GET failed: #{path}, status #{response.code}" unless response.code == '200'
  [JSON.parse(response.body), response]
end
health, response = get.call('/health')
raise 'Runtime version mismatch' unless health['version'] == expected && response['x-app-version'] == expected
plugins, = get.call('/api/plugins')
todo = plugins.fetch('plugins').find { |plugin| plugin['id'] == 'willdeep-todo' }
raise 'Todo package unavailable' unless todo && todo['version'] == '1.5.0' && todo['enabled'] && todo['approval_gap'].nil?
commands = todo.fetch('commands').map { |entry| entry.fetch('id') }
%w[todo.attach todo.attachment_preview todo.detach].each do |name|
  raise "Command missing: #{name}" unless commands.include?(name)
end
store = JSON.parse(File.read(metadata.fetch('todo_store')))
images = store.fetch('items').flat_map do |item|
  Array(item['attachments']).filter_map do |attachment|
    next unless attachment['type'].to_s.start_with?('image/')
    { id: item.fetch('id'), attachmentID: attachment.fetch('id') }
  end
end
images.each do |arguments|
  result = Net::HTTP.start(base.host, base.port, read_timeout: 15) do |http|
    http.post('/api/plugins/willdeep-todo/commands/todo.attachment_preview', JSON.generate(arguments: arguments), 'content-type' => 'application/json')
  end
  raise "Live preview failed with HTTP #{result.code}" unless result.code == '200'
  value = JSON.parse(result.body).fetch('result')
  value = JSON.parse(value.fetch('content').find { |entry| entry['type'] == 'text' }.fetch('text')) unless value.key?('ok')
  raise "Live preview rejected: #{value['error']}" unless value['ok'] && value['mimeType'].to_s.start_with?('image/') && !Base64.decode64(value.fetch('dataBase64')).empty?
end
before = JSON.parse(File.read(File.join(metadata.fetch('backup_dir'), 'plugin-registry.web.json'))).fetch('plugins')
after = JSON.parse(File.read(File.expand_path('~/.willdeep/plugin-registry.web.json'))).fetch('plugins')
others_unchanged = before.reject { |key, _| key == 'willdeep-todo' } == after.reject { |key, _| key == 'willdeep-todo' }
unchanged = Digest::SHA256.file(metadata.fetch('todo_store')).hexdigest == metadata.fetch('todo_before_sha256')
raise 'Todo data changed during update; manual comparison required' unless unchanged
sessions, = get.call('/api/sessions')
report = { passed: true, url: base.to_s, runtime_version: health['version'], http_app_version: response['x-app-version'],
  plugin_version: todo['version'], attachment_commands_registered: true, live_image_previews_passed: images.size,
  todo_count: store.fetch('items').size, todo_file_unchanged: unchanged,
  other_plugin_registrations_unchanged: others_unchanged, session_count: sessions.size,
  note: 'No todo was created, edited or deleted during live verification.' }
directory = File.expand_path('../build/verification', __dir__)
FileUtils.mkdir_p(directory)
File.write(File.join(directory, 'live-runtime-update.json'), JSON.pretty_generate(report))
File.write(File.join(directory, 'live-runtime-update.md'), "# 9847 实际运行版本验收\n\n" + report.map { |key, value| "- #{key}: #{value}\n" }.join)
puts JSON.generate(report)
