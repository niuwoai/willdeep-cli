#!/usr/bin/env ruby
# frozen_string_literal: true

# npm 安装包的离线契约测试。
#
#   ruby scripts/test/npm_willdeep_test.rb
#
# 不访问 GitHub、不执行 npm publish。测试用本地假 HTTP 服务和假发行包钉住平台选择、
# SHA256、压缩包边界、下载失败和重复安装。

require 'digest'
require 'fileutils'
require 'json'
require 'minitest/autorun'
require 'open3'
require 'tmpdir'
require 'webrick'

ROOT = File.expand_path('../..', __dir__)
PACKAGE = File.join(ROOT, 'npm', 'willdeep')

class NpmWilldeepTest < Minitest::Test
  def setup
    @server = nil
  end

  def teardown
    @server&.shutdown
  end

  def test_postinstall_waits_for_failure_and_exits_nonzero
    Dir.mktmpdir do |directory|
      copy_package(directory)
      File.write(File.join(directory, 'install-manifest.json'), '{')
      _out, err, status = Open3.capture3('node', 'lib/install.js', chdir: directory)
      refute status.success?
      assert_includes utf8(err), 'SyntaxError'
    end
  end

  def test_manifest_matches_package_and_release_names
    package = JSON.parse(File.read(File.join(PACKAGE, 'package.json')))
    manifest = JSON.parse(File.read(File.join(PACKAGE, 'install-manifest.json')))
    assert_equal '@niuwoai/willdeep', package['name']
    assert_equal package['version'], manifest['version']
    assert_equal "v#{package['version']}", manifest['tag']
    assert_equal 'niuwoai/willdeep-cli', manifest['repository']
    assert_equal({ 'willdeep' => 'bin/willdeep.js' }, package['bin'])
    assert_equal 'rc', package.dig('publishConfig', 'tag')
  end

  def test_install_verifies_archive_and_reuses_exact_binary
    binary = File.binread('/bin/echo')
    archive = Dir.mktmpdir do |directory|
      File.write(File.join(directory, 'willdeep'), binary, perm: 0o755)
      _out, err, status = Open3.capture3('tar', '-czf', 'asset.tar.gz', 'willdeep', chdir: directory)
      flunk(err) unless status.success?
      File.binread(File.join(directory, 'asset.tar.gz'))
    end
    digest = Digest::SHA256.hexdigest(archive)
    serve('willdeep-linux-amd64.tar.gz' => archive)
    manifest = manifest_for(digest)

    Dir.mktmpdir do |directory|
      copy_package(directory)
      File.write(File.join(directory, 'install-manifest.json'), JSON.generate(manifest))
      out, err, status = run_installer(directory, 'linux', 'x64')
      assert status.success?, "#{out}\n#{err}"
      installed = File.join(directory, 'bin', 'willdeep')
      assert_equal binary, File.binread(installed)
      assert_equal 0o755, File.stat(installed).mode & 0o777

      File.write(installed, 'changed')
      second_out, second_err, second_status = run_installer(directory, 'linux', 'x64')
      assert second_status.success?, "#{second_out}\n#{second_err}"
      assert_equal 'changed', File.read(installed), '同版本同校验值不得覆盖已安装程序'
    end
  end

  def test_wrong_digest_archive_shape_and_unsupported_platform_fail
    Dir.mktmpdir do |directory|
      copy_package(directory)
      File.write(File.join(directory, 'willdeep'), 'x', perm: 0o755)
      File.write(File.join(directory, 'extra'), 'y')
      _out, err, status = Open3.capture3('tar', '-czf', 'bad.tar.gz', 'willdeep', 'extra', chdir: directory)
      flunk(err) unless status.success?
      archive = File.binread(File.join(directory, 'bad.tar.gz'))
      serve('willdeep-linux-amd64.tar.gz' => archive)
      File.write(File.join(directory, 'install-manifest.json'), JSON.generate(manifest_for('0' * 64)))

      _out, err, status = run_installer(directory, 'linux', 'x64')
      refute status.success?
      assert_includes utf8(err), 'SHA256'

      manifest = manifest_for(Digest::SHA256.hexdigest(archive))
      File.write(File.join(directory, 'install-manifest.json'), JSON.generate(manifest))
      _out, err, status = run_installer(directory, 'linux', 'x64')
      refute status.success?
      assert_includes utf8(err), '内容异常'

      _out, err, status = run_installer(directory, 'freebsd', 'x64')
      refute status.success?
      assert_includes utf8(err), '暂不支持'
    end
  end

  def test_symlink_and_windows_archive_without_mode_are_handled
    Dir.mktmpdir do |directory|
      copy_package(directory)
      File.symlink('/bin/echo', File.join(directory, 'willdeep'))
      _out, err, status = Open3.capture3('tar', '-czf', 'link.tar.gz', 'willdeep', chdir: directory)
      flunk(err) unless status.success?
      archive = File.binread(File.join(directory, 'link.tar.gz'))
      serve('willdeep-linux-amd64.tar.gz' => archive)
      File.write(File.join(directory, 'install-manifest.json'), JSON.generate(manifest_for(Digest::SHA256.hexdigest(archive))))
      _out, err, status = run_installer(directory, 'linux', 'x64')
      refute status.success?
      assert_includes utf8(err), '普通文件'

      exe = File.binread('/bin/echo')
      zip = Dir.mktmpdir do |source|
        File.binwrite(File.join(source, 'willdeep.exe'), exe)
        _out, zip_err, zip_status = Open3.capture3('python3', '-c', 'import pathlib,zipfile; zipfile.ZipFile("win.zip","w").write("willdeep.exe")', chdir: source)
        flunk(zip_err) unless zip_status.success?
        File.binread(File.join(source, 'win.zip'))
      end
      serve('willdeep-windows-x64.zip' => zip)
      manifest = manifest_for(Digest::SHA256.hexdigest(zip))
      manifest['assets'] = {
        'willdeep-windows-x64.zip' => { 'name' => 'willdeep-windows-x64.zip', 'sha256' => Digest::SHA256.hexdigest(zip) }
      }
      File.write(File.join(directory, 'install-manifest.json'), JSON.generate(manifest))
      _out, err, status = run_installer(directory, 'win32', 'x64')
      assert status.success?, "#{_out}\n#{utf8(err)}"
      assert_equal exe, File.binread(File.join(directory, 'bin', 'willdeep.exe'))
    end
  end

  def test_redirect_limit_and_malformed_manifest_fail_closed
    oversized = 'x' * 1024
    serve('willdeep-linux-amd64.tar.gz' => oversized)
    @server.mount_proc('/redirect') { |_request, response| response.status = 302; response['Location'] = '/redirect' }
    Dir.mktmpdir do |directory|
      copy_package(directory)
      manifest = manifest_for(Digest::SHA256.hexdigest(oversized))
      manifest['repository'] = 'example.test/../redirect'
      File.write(File.join(directory, 'install-manifest.json'), JSON.generate(manifest))
      _out, err, status = run_installer(directory, 'linux', 'x64')
      refute status.success?
      assert_includes utf8(err), '安装清单不完整'

      manifest['repository'] = 'example.test/willdeep'
      manifest['assets']['willdeep-linux-amd64.tar.gz']['sha256'] = 'not-a-digest'
      File.write(File.join(directory, 'install-manifest.json'), JSON.generate(manifest))
      _out, err, status = run_installer(directory, 'linux', 'x64')
      refute status.success?
      assert_includes utf8(err), '校验值'

      @server.mount_proc('/example.test/willdeep/releases/download/v0.123.0-rc2/willdeep-linux-amd64.tar.gz') do |_request, response|
        response.status = 302
        response['Location'] = 'https://github.com:8443/unrelated'
      end
      manifest = manifest_for(Digest::SHA256.hexdigest('x'))
      File.write(File.join(directory, 'install-manifest.json'), JSON.generate(manifest))
      _out, err, status = run_installer(directory, 'linux', 'x64')
      refute status.success?
      assert_includes utf8(err), '不受信任'
    end
  end

  private

  def utf8(text)
    text.to_s.dup.force_encoding(Encoding::UTF_8)
  end

  def copy_package(directory)
    FileUtils.cp_r(Dir[File.join(PACKAGE, '*')], directory)
  end

  def manifest_for(digest)
    {
      'version' => '0.123.0-rc2',
      'repository' => 'example.test/willdeep',
      'tag' => 'v0.123.0-rc2',
      'assets' => {
        'willdeep-linux-amd64.tar.gz' => {
          'name' => 'willdeep-linux-amd64.tar.gz',
          'sha256' => digest
        }
      }
    }
  end

  def serve(files)
    @server = WEBrick::HTTPServer.new(Port: 0, Logger: WEBrick::Log.new(File::NULL), AccessLog: [])
    files.each do |name, body|
      @server.mount_proc("/example.test/willdeep/releases/download/v0.123.0-rc2/#{name}") do |_request, response|
        response.status = 200
        response['Content-Type'] = 'application/octet-stream'
        response.body = body
      end
    end
    Thread.new { @server.start }
  end

  def run_installer(directory, platform, arch)
    script = <<~'JS'
      const { installWilldeep } = require('./lib/release');
      const fs = require('node:fs');
      const manifest = JSON.parse(fs.readFileSync('install-manifest.json', 'utf8'));
      const base = `http://127.0.0.1:${process.argv[3]}`;
      const fetchImpl = (url, options) => fetch(url.replace('https://github.com', base), options);
      installWilldeep({
        manifest,
        platform: process.argv[1],
        arch: process.argv[2],
        destination: require('node:path').join(process.cwd(), 'bin', process.argv[1] === 'win32' ? 'willdeep.exe' : 'willdeep'),
        packageRoot: process.cwd(),
        fetchImpl,
        trustedHosts: new Set(['127.0.0.1']),
      }).catch((error) => {
        process.stderr.write(`${error.stack}\n`);
        process.exit(1);
      });
    JS
    Open3.capture3('node', '-e', script, platform, arch, @server.config[:Port].to_s, chdir: directory)
  end
end
