# frozen_string_literal: true

# Pack exactly the local source, audit every archive member, then install the
# extracted packages in the real host. No registry or dependency installation.
require_relative 'check-host'
require 'rubygems/package'
require 'zlib'

class PackageCheck
  attr_reader :report

  def initialize(binary)
    @binary = binary
    @report = { format: 'willdeep.magpie-package-check.v1', status: 'running', packages: [], host: nil }
  end

  def run
    Dir.mktmpdir('willdeep-magpie-packages-') do |temporary|
      root = File.join(temporary, 'packages')
      tool_home = File.join(temporary, 'tool-home')
      FileUtils.mkdir_p([root, tool_home])
      node = executable('node')
      yarn = executable('yarn')
      environment = {
        'HOME' => tool_home, 'USERPROFILE' => tool_home, 'TMPDIR' => tool_home,
        'XDG_CONFIG_HOME' => tool_home, 'XDG_CACHE_HOME' => tool_home, 'XDG_DATA_HOME' => tool_home,
        'PATH' => [File.dirname(node), File.dirname(yarn), '/usr/bin', '/bin'].uniq.join(File::PATH_SEPARATOR),
        'YARN_IGNORE_PATH' => '1', 'YARN_DISABLE_SELF_UPDATE_CHECK' => '1', 'NO_COLOR' => '1'
      }
      CLIENTS.each_value do |directory|
        source = File.join(PACKAGES_ROOT, directory)
        manifest = JSON.parse(File.read(File.join(source, 'package.json')))
        archive = File.join(temporary, "#{directory}-#{manifest.fetch('version')}.tgz")
        run_tool(environment, yarn, '--cwd', source, 'pack', '--filename', archive, chdir: tool_home)
        destination = File.join(root, directory)
        files = unpack(archive, source, destination, manifest.fetch('files') + ['package.json'])
        output = run_tool(environment, node, '--test', '--test-reporter=tap',
                          File.join(destination, 'test/agent.test.js'), chdir: destination)
        tests = output[/^# pass (\d+)$/, 1]
        raise CheckFailure, "Missing native test result for #{directory}: #{output}" unless tests
        @report[:packages] << { directory: directory, version: manifest.fetch('version'),
                               archive: File.join(PACKAGES_ROOT, File.basename(archive)),
                               sha256: Digest::SHA256.file(archive).hexdigest,
                               files: files.sort, native_tests_passed: tests.to_i }
      end
      home = File.join(temporary, 'host-home')
      FileUtils.mkdir_p(home)
      check = HostCheck.new(@binary, home, packages_root: root)
      @report[:host] = check.report
      check.run
      # Only expose generated archives after the exact extracted bytes passed.
      @report[:packages].each do |package|
        FileUtils.cp(File.join(temporary, File.basename(package[:archive])), package[:archive])
      end
      @report[:status] = 'passed'
    end
  rescue StandardError => error
    @report[:status] = 'failed'
    @report[:error] = error.message
    raise
  end

  private

  def executable(name)
    ENV.fetch('PATH', '').split(File::PATH_SEPARATOR).each do |directory|
      path = File.join(directory, name)
      return File.expand_path(path) if File.file?(path) && File.executable?(path)
    end
    raise CheckFailure, "An existing #{name} executable is required; nothing is installed"
  end

  def run_tool(environment, *arguments, chdir:)
    output = nil
    status = nil
    Open3.popen2e(environment, *arguments, unsetenv_others: true, chdir: chdir, pgroup: true) do |input, stream, wait|
      input.close
      stream.set_encoding(Encoding::UTF_8)
      reader = Thread.new { stream.read }
      begin
        Timeout.timeout(30) { status = wait.value }
      rescue Timeout::Error
        Process.kill('KILL', -wait.pid)
        wait.value
        raise CheckFailure, "Timed out: #{File.basename(arguments.first)}"
      ensure
        output = reader.value
      end
    end
    raise CheckFailure, "#{File.basename(arguments.first)} exited #{status.exitstatus}: #{output}" unless status.success?
    output
  end

  def unpack(archive, source, destination, expected)
    files = []
    FileUtils.mkdir_p(destination)
    Zlib::GzipReader.open(archive) do |gzip|
      Gem::Package::TarReader.new(gzip) do |tar|
        tar.each do |entry|
          next if entry.directory?
          relative = entry.full_name.delete_prefix('package/')
          unless entry.file? && entry.full_name.start_with?('package/') && expected.include?(relative) && !files.include?(relative)
            raise CheckFailure, "Unexpected archive member: #{entry.full_name}"
          end
          bytes = entry.read
          raise CheckFailure, "Archive differs from source: #{relative}" unless bytes == File.binread(File.join(source, relative))
          path = File.join(destination, relative)
          FileUtils.mkdir_p(File.dirname(path))
          File.binwrite(path, bytes)
          files << relative
        end
      end
    end
    raise CheckFailure, "Archive member list differs for #{File.basename(source)}" unless files.sort == expected.sort
    files
  end
end

if $PROGRAM_NAME == __FILE__
  format = :json
  OptionParser.new { |parser| parser.on('--markdown') { format = :markdown } }.parse!
  binary = ENV['MAGPIE_BIN']
  abort 'Set MAGPIE_BIN to an existing Magpie executable; nothing is downloaded or installed.' unless binary && File.executable?(binary)
  check = PackageCheck.new(binary)
  exit_status = 0
  begin
    check.run
  rescue StandardError
    # PackageCheck retains tool output or the failed host case in its report.
    exit_status = 1
  ensure
    report = check.report
    if format == :json
      puts JSON.pretty_generate(report)
    else
      puts "# Magpie 本地归档验证\n\n- 结果：#{report[:status]}\n- 宿主：#{report.dig(:host, :host_version)}\n- 模型请求：#{report.dig(:host, :model_requests)}\n"
      puts "\n| 包 | 版本 | 文件数 | 原生测试 | SHA-256 |\n| --- | --- | --- | --- | --- |"
      report[:packages].each do |package|
        puts "| #{package[:directory]} | #{package[:version]} | #{package[:files].length} | #{package[:native_tests_passed]} | #{package[:sha256]} |"
      end
      puts "\n| 真实宿主场景 | 结果 |\n| --- | --- |"
      report.fetch(:host, nil)&.fetch(:cases)&.each { |item| puts "| #{item[:name]} | #{item[:status]} |" }
      puts "\n错误：#{report[:error]}" if report[:error]
    end
  end
  exit exit_status
end
