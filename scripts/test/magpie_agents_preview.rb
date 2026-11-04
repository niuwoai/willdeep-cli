#!/usr/bin/env ruby
# Browser verification fixture: all configuration and model traffic stays local.
require_relative 'magpie_agents_runtime'

def stop_preview_child(pid)
  return unless pid
  return if Process.waitpid(pid, Process::WNOHANG)
  Process.kill('TERM', -pid)
  begin
    Timeout.timeout(3) { Process.waitpid(pid) }
  rescue Timeout::Error
    Process.kill('KILL', -pid)
    Process.waitpid(pid)
  end
rescue Errno::ESRCH, Errno::ECHILD
  # The owned child has already exited.
end

root = Dir.mktmpdir('wd-magpie-preview-', '/private/tmp')
home = File.join(root, 'user')
config_home = File.join(home, '.willdeep')
workspace = File.join(root, 'workspace')
FileUtils.mkdir_p([config_home, workspace])
fixture = MagpieAgentFixture.new
environment = {
  'PATH' => ENV.fetch('PATH'), 'HOME' => home, 'USERPROFILE' => home,
  'WILLDEEP_HOME' => config_home, 'WILLDEEP_LANGUAGE' => 'zh-CN',
  'WILLDEEP_TELEMETRY_DISABLED' => '1', 'DO_NOT_TRACK' => '1', 'WILLDEEP_LOCAL_DISCOVERY' => '0',
  'XDG_CONFIG_HOME' => File.join(home, '.config'),
  'XDG_CACHE_HOME' => File.join(home, '.cache'),
  'XDG_DATA_HOME' => File.join(home, '.local/share'), 'LANG' => 'en_US.UTF-8'
}
config = File.join(config_home, 'config.toml')
File.write(config, "version=1\ndefault_provider='native'\n[agent]\nauto_title=false\nmax_turns=2\napproval='smart'\n[providers.native]\nprovider='openai-compatible'\napi='chat-completions'\napi_base='#{fixture.base_url}'\napi_key='mock-native'\nmodel='native/model'\n")
File.chmod(0o600, config)
connection = File.join(config_home, 'integrations/magpie/cli/connection.json')
FileUtils.mkdir_p(File.dirname(connection))
File.write(connection, JSON.generate(schema_version: 1, client_id: 'willdeep-cli', enabled: true,
  base_url: fixture.base_url, client_marker: 'magpie-willdeep-cli', model: 'mock/model-a',
  context_window: 32768, max_output_tokens: 4096))
binary = File.expand_path(ARGV.fetch(0, 'target/debug/willdeep'))
port = Integer(ARGV.fetch(1, '19847'))
stopping = false
%w[INT TERM].each { |signal| Signal.trap(signal) { stopping = true } }
web = nil
exit_code = 0
begin
  web = Process.spawn(environment, binary, '--web', '--listen', "127.0.0.1:#{port}",
                      '--workspace', workspace, unsetenv_others: true, pgroup: true)
  Timeout.timeout(20) do
    loop do
      if Process.waitpid(web, Process::WNOHANG)
        exit_code = $?.exitstatus || 1
        web = nil
        raise 'Web process exited before becoming ready'
      end
      begin
        response = Net::HTTP.start('127.0.0.1', port, nil, open_timeout: 1, read_timeout: 1) do |http|
          http.get('/api/models/default')
        end
        break if response.code == '200' && JSON.parse(response.body).dig('model', 'model') == 'mock/model-a'
      rescue Errno::ECONNREFUSED, EOFError, Net::OpenTimeout, Net::ReadTimeout
        # The isolated Web process has not bound its listener yet.
      end
      sleep 0.1
    end
  end
  puts "Isolated Magpie browser fixture: http://127.0.0.1:#{port}"
  puts "Fixture root: #{root}"
  STDOUT.flush
  until stopping
    if Process.waitpid(web, Process::WNOHANG)
      exit_code = $?.exitstatus || 1
      web = nil
      break
    end
    sleep 0.2
  end
rescue StandardError => error
  warn "preview failed: #{error.message}"
  exit_code = 1
ensure
  begin
    stop_preview_child(web)
    Open3.popen3(environment, binary, 'daemon', 'stop', unsetenv_others: true, pgroup: true) do |input, output, errors, child|
      input.close
      readers = [Thread.new { output.read }, Thread.new { errors.read }]
      begin
        Timeout.timeout(15) do
          status = child.value
          diagnostics = readers.map(&:value).join
          unless status.success?
            warn "fixture Runtime cleanup failed: #{diagnostics}"
            exit_code = 1
          end
        end
      ensure
        stop_preview_child(child.pid)
        readers.each { |reader| reader.kill if reader.alive? }
      end
    end
  rescue StandardError => error
    warn "preview cleanup failed: #{error.message}"
    exit_code = 1
  ensure
    fixture.close
    FileUtils.remove_entry(root)
  end
end
exit exit_code
