# frozen_string_literal: true

# Real Magpie plugin registration, moejs connect, atomic writes and restoration.
# All subprocess configuration, discovery, PATH and network targets are synthetic.
# Existing Magpie/Bun executables are used without modifying their installation.
require 'json'
require 'tmpdir'
require 'fileutils'
require 'open3'
require 'timeout'
require 'socket'
require 'digest'
require 'optparse'

PACKAGES_ROOT = File.expand_path('..', __dir__)
CLIENTS = { 'willdeep' => 'agent-willdeep', 'willdeep-cli' => 'agent-willdeep-cli' }.freeze

class CheckFailure < StandardError; end

class HostCheck
  attr_reader :report

  def initialize(binary, home, packages_root: PACKAGES_ROOT)
    @binary = File.realpath(binary)
    @home = home
    @packages_root = File.realpath(packages_root)
    @requests = []
    @server = TCPServer.new('127.0.0.1', 0)
    @port = @server.addr[1]
    @upstream = Thread.new { serve }
    @environment = {
      'HOME' => home, 'USERPROFILE' => home, 'XDG_CONFIG_HOME' => File.join(home, '.config'),
      'XDG_CACHE_HOME' => File.join(home, '.cache'), 'XDG_DATA_HOME' => File.join(home, '.local/share'),
      'TMPDIR' => File.join(home, 'tmp'), 'PATH' => File.join(home, 'bin'),
      'MAGPIE_TEST_SANDBOX' => home, 'MAGPIE_PARTNERS' => 'off', 'MAGPIE_NO_ACCOUNTS' => '1',
      'MAGPIE_BUN' => ENV.fetch('MAGPIE_BUN', '/usr/bin/false'), 'MAGPIE_ADDR' => "127.0.0.1:#{@port}",
      'MAGPIE_PLUGIN_MARKET' => "http://127.0.0.1:#{@port}/market.json",
      'HTTP_PROXY' => "http://127.0.0.1:#{@port}", 'HTTPS_PROXY' => "http://127.0.0.1:#{@port}",
      'NO_PROXY' => 'localhost,127.0.0.1,::1', 'NO_COLOR' => '1', 'LANG' => 'en_US.UTF-8'
    }
    FileUtils.mkdir_p([@environment['TMPDIR'], @environment['PATH']])
    @report = { format: 'willdeep.magpie-host-check.v1', host_version: nil,
                host_binary_sha256: Digest::SHA256.file(@binary).hexdigest,
                contract_source_revision: '759b4d1d7da8b983860539e3bd8ce1eaf70a06a4', status: 'running',
                cases: [], model_requests: 0, home_is_temporary: true }
  end

  def run
    @report[:host_version] = command('version').strip
    CLIENTS.each_value { |directory| command('plugin', 'add', File.join(@packages_root, directory)) }
    check('both local packages register in the real host') do
      list = command('plugin', 'list', '--json')
      CLIENTS.each_value { |directory| expect(list.include?(directory), "Missing #{directory}") }
      agents = command('agents')
      ['WillDeep', 'WillDeep CLI'].each { |name| expect(agents.include?(name), "Unregistered #{name}") }
      CLIENTS.each_key { |id| command(id) }
      expect(!list.match?(/didn.t load|outside the plugin|SyntaxError/), 'Host module load failed')
    end

    check('neither shared root nor package registration detects either client') do
      FileUtils.mkdir_p(File.join(@home, '.willdeep'))
      detected = command('ls', expected_status: 1)
      expect(detected.include?('no supported agents found'), 'Unexpected empty-discovery failure')
      expect(!detected.include?('WillDeep'), 'Shared root or plugin registration incorrectly detected a client')
    end
    check('CLI executable detection does not detect Desktop') do
      executable = File.join(@environment['PATH'], 'willdeep')
      # Only test PATH discovery; never execute or install a real client.
      FileUtils.ln_s('/usr/bin/true', executable)
      begin
        detected = command('ls')
        expect(detected.include?('WillDeep CLI'), 'Synthetic CLI executable was not detected')
        expect(!detected.match?(/WillDeep(?! CLI)/), 'CLI executable incorrectly detected Desktop')
      ensure
        File.unlink(executable)
      end
    end

    support = File.join(@home, '.willdeep/integrations/magpie/desktop')
    FileUtils.mkdir_p(support)
    File.write(File.join(support, 'client.json'), JSON.generate(client_id: 'willdeep', version: '1.427.0-rc1'))
    check('Desktop support never detects a missing CLI') do
      detected = command('ls')
      expect(detected.include?('WillDeep'), "Desktop support directory was not detected: #{detected}")
      expect(!detected.include?('WillDeep CLI'), 'Desktop directory incorrectly detected CLI')
    end

    command('provider', 'add', 'mock', "url=http://127.0.0.1:#{@port}/v1", 'key=local-test-marker', 'models=model-a,model-b')
    CLIENTS.each_key do |id|
      check("#{id}: real moejs connect preserves model A and all seven connection fields") do
        command(id, 'magpie/mock/model-a')
        validate_connection(id, 'mock/model-a')
      end
    end
    desktop_before = File.binread(connection_path('willdeep'))
    check('CLI model B does not change Desktop model A') do
      command('willdeep-cli', 'magpie/mock/model-b')
      validate_connection('willdeep-cli', 'mock/model-b')
      expect(File.binread(connection_path('willdeep')) == desktop_before, 'CLI changed Desktop file')
    end
    cli_before = File.binread(connection_path('willdeep-cli'))
    check('Desktop disconnect restores an empty object without changing CLI') do
      command('willdeep', 'default')
      expect(read_connection('willdeep').empty?, 'Desktop connection keys survived disconnect')
      expect(File.binread(connection_path('willdeep-cli')) == cli_before, 'Desktop changed CLI file')
    end
    check('CLI native-model restoration leaves only model, a disconnected state') do
      command('willdeep-cli', 'native/model')
      expect(read_connection('willdeep-cli') == { 'model' => 'native/model' }, 'Unexpected restored native-model fields')
    end
    check('reconnect and default remove managed keys on both clients') do
      CLIENTS.each_key do |id|
        previous = read_connection(id)
        command(id, 'magpie/mock/model-b')
        validate_connection(id, 'mock/model-b')
        command(id, 'default')
        expect(read_connection(id) == previous, "Managed keys survived #{id} disconnect")
      end
    end
    check('custom CLI home uses a matching local descriptor and leaves standard home untouched') do
      source = File.join(@packages_root, 'agent-willdeep-cli')
      local = File.join(@home, 'custom-cli-plugin')
      FileUtils.cp_r(source, local)
      entry = File.join(local, 'willdeep-cli.agent.js')
      code = File.read(entry).sub('~/.willdeep/integrations/magpie/cli/connection.json', '$WILLDEEP_HOME/integrations/magpie/cli/connection.json')
      File.write(entry, code)
      command('plugin', 'rm', source)
      command('plugin', 'add', local)
      @environment['WILLDEEP_HOME'] = File.join(@home, 'custom-willdeep-home')
      standard = File.binread(connection_path('willdeep-cli'))
      command('willdeep-cli', 'magpie/mock/model-a')
      custom_path = File.join(@environment['WILLDEEP_HOME'], 'integrations/magpie/cli/connection.json')
      custom = JSON.parse(File.read(custom_path))
      expect(custom['model'] == 'mock/model-a', 'Custom home was not used')
      expect(File.binread(connection_path('willdeep-cli')) == standard, 'Custom connection wrote standard home')
      command('willdeep-cli', 'default')
      expect(JSON.parse(File.read(custom_path)).empty?, 'Custom home did not disconnect')
    end
    check('no model request or runtime installation occurred') do
      calls = @requests.select { |path| path.include?('/chat/completions') || path.include?('/responses') || path.include?('generateContent') }
      @report[:model_requests] = calls.length
      expect(calls.empty?, 'Unexpected model request')
      CLIENTS.each_key do |id|
        expect(!File.exist?(File.join(@home, '.willdeep/config.toml')), "#{id} created the CLI main configuration")
      end
      bun_paths = Dir.glob(File.join(@home, '**/bun'), File::FNM_DOTMATCH).select { |path| File.file?(path) && File.executable?(path) }
      expect(bun_paths.empty?, "Bun was installed: #{bun_paths.join(', ')}")
    end
    @report[:status] = 'passed'
  rescue StandardError => error
    @report[:status] = 'failed'
    @report[:error] = error.message
    raise
  ensure
    @report[:local_http_requests] = @requests.length
    @server.close
    @upstream.join(2)
  end

  private

  def check(name)
    item = { name: name, status: 'running' }
    @report[:cases] << item
    yield
    item[:status] = 'passed'
  rescue StandardError => error
    item[:status] = 'failed'
    item[:error] = error.message
    raise
  end

  def command(*arguments, expected_status: 0)
    output = errors = nil
    status = nil
    Open3.popen3(@environment, @binary, *arguments, unsetenv_others: true, chdir: @home, pgroup: true) do |input, stdout, stderr, wait|
      input.close
      stdout.set_encoding(Encoding::UTF_8)
      stderr.set_encoding(Encoding::UTF_8)
      out_reader = Thread.new { stdout.read }
      err_reader = Thread.new { stderr.read }
      begin
        Timeout.timeout(30) { status = wait.value }
      rescue Timeout::Error
        Process.kill('KILL', -wait.pid)
        wait.value
        raise CheckFailure, "Timed out: #{arguments.first(2).join(' ')}"
      ensure
        output = out_reader.value
        errors = err_reader.value
      end
    end
    unless status.exitstatus == expected_status
      raise CheckFailure, "#{arguments.first(2).join(' ')} exited #{status.exitstatus} (expected #{expected_status}): #{errors.strip}"
    end
    expected_status.zero? ? output : output + errors
  end

  def connection_path(id)
    File.join(@home, '.willdeep/integrations/magpie', id == 'willdeep' ? 'desktop' : 'cli', 'connection.json')
  end

  def read_connection(id)
    JSON.parse(File.read(connection_path(id)))
  end

  def validate_connection(id, model)
    connection = read_connection(id)
    expected = { 'schema_version' => 1, 'client_id' => id, 'enabled' => true,
                 'base_url' => "http://127.0.0.1:#{@port}/v1", 'client_marker' => "magpie-#{id}",
                 'model' => model, 'context_window' => nil, 'max_output_tokens' => nil }
    expect(connection == expected, "#{id} connection differs from the exact v1 fields: #{connection.keys.sort.join(', ')}")
  end

  def expect(condition, message)
    raise CheckFailure, message unless condition
  end

  def serve
    loop do
      socket = @server.accept
      socket.read_timeout = 5 if socket.respond_to?(:read_timeout=)
      first = socket.gets
      next unless first
      path = first.split(' ')[1]
      @requests << path
      content_length = 0
      while (line = socket.gets) && line != "\r\n"
        content_length = line.split(':', 2).last.to_i if line.downcase.start_with?('content-length:')
      end
      socket.read(content_length) if content_length.positive?
      body = if path.include?('/models')
               JSON.generate(object: 'list', data: %w[model-a model-b].map { |id| { id: id, object: 'model' } })
             elsif path.include?('market.json')
               JSON.generate(plugins: [])
             else
               '{}'
             end
      socket.write("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: #{body.bytesize}\r\nConnection: close\r\n\r\n#{body}")
      socket.close
    end
  rescue IOError, Errno::EBADF
    # Closing the test listener ends the fixture server.
    nil
  end
end

if $PROGRAM_NAME == __FILE__
  options = { packages_root: PACKAGES_ROOT, format: :json }
  OptionParser.new do |parser|
    parser.on('--markdown') { options[:format] = :markdown }
    parser.on('--packages-root PATH') { |path| options[:packages_root] = File.expand_path(path) }
  end.parse!
  binary = ENV['MAGPIE_BIN']
  abort 'Set MAGPIE_BIN to an existing Magpie executable; no download or installation is performed.' unless binary && File.executable?(binary)
  exit_status = 0
  Dir.mktmpdir('willdeep-magpie-host-') do |home|
    check = HostCheck.new(binary, home, packages_root: options[:packages_root])
    begin
      check.run
    rescue StandardError
      # HostCheck retains the failed case and error in the structured report.
      exit_status = 1
    ensure
      report = check.report
      if options[:format] == :json
        puts JSON.pretty_generate(report)
      else
        puts "# Magpie Agent 宿主验证\n\n- 宿主：#{report[:host_version]}\n- 结果：#{report[:status]}\n- 模型请求：#{report[:model_requests]}\n- 宿主 SHA-256：#{report[:host_binary_sha256]}\n- 契约源码基线：#{report[:contract_source_revision]}\n"
        puts "\n| 场景 | 结果 |\n| --- | --- |"
        report[:cases].each { |item| puts "| #{item[:name]} | #{item[:status]} |" }
        puts "\n错误：#{report[:error]}" if report[:error]
      end
    end
  end
  exit exit_status
end
