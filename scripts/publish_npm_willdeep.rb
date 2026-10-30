#!/usr/bin/env ruby
# frozen_string_literal: true

# 把当前 npm/willdeep 包发布到 npm。
#
#   ruby scripts/publish_npm_willdeep.rb --version 0.123.0-rc2
#
# 它只接受与 package.json、install-manifest.json 完全一致的版本，先下载四个
# GitHub Release 产物并核对 SHA256，再执行 npm publish --tag rc。

require 'digest'
require 'json'
require 'net/http'
require 'open3'
require 'optparse'
require 'tmpdir'
require 'uri'

ROOT = File.expand_path('..', __dir__)
PACKAGE = File.join(ROOT, 'npm', 'willdeep')
ASSETS = {
  'willdeep-macos-universal.tar.gz' => 'willdeep',
  'willdeep-linux-amd64.tar.gz' => 'willdeep',
  'willdeep-linux-arm64.tar.gz' => 'willdeep',
  'willdeep-windows-x64.zip' => 'willdeep.exe'
}.freeze
MAX_BYTES = 128 * 1024 * 1024
TRUSTED_HOSTS = %w[github.com release-assets.githubusercontent.com].freeze

def read_json(path)
  JSON.parse(File.read(path))
rescue JSON::ParserError, Errno::ENOENT => error
  abort "#{path} 无法读取：#{error.message}"
end

def trusted_redirect?(target)
  # URI 会把 https 的默认端口填成 443。GitHub Release 现在就跳到带 :443 的地址，
  # 只认 port.nil? 会把这条合法重定向拒掉。显式非默认端口仍然拒绝。
  target.scheme == 'https' &&
    target.port == URI::HTTPS.default_port &&
    TRUSTED_HOSTS.include?(target.host)
end

def follow(uri, limit = 5)
  raise "重定向过多：#{uri}" if limit.negative?

  Net::HTTP.start(uri.host, uri.port, use_ssl: uri.scheme == 'https') do |http|
    response = http.request(Net::HTTP::Get.new(uri))
    return response unless response.is_a?(Net::HTTPRedirection)

    location = response['location']
    raise '空的重定向' if location.nil? || location.empty?

    target = URI.join(uri, location)
    raise "不受信任的重定向：#{target.host}" unless trusted_redirect?(target)
    follow(target, limit - 1)
  end
end

def verify_assets(manifest)
  assets = {}
  ASSETS.each do |name, entry|
    uri = URI("https://github.com/#{manifest['repository']}/releases/download/#{manifest['tag']}/#{name}")
    response = follow(uri)
    abort "#{name} 下载失败：HTTP #{response.code}" unless response.is_a?(Net::HTTPSuccess)
    body = response.body
    abort "#{name} 超过 128 MiB" if body.bytesize > MAX_BYTES
    digest = Digest::SHA256.hexdigest(body)
    Dir.mktmpdir do |directory|
      archive = File.join(directory, name)
      File.binwrite(archive, body)
      output = File.join(directory, 'out')
      Dir.mkdir(output)
      command = name.end_with?('.zip') ? ['unzip', '-q', archive, '-d', output] : ['tar', '-xzf', archive, '-C', output]
      _out, err, status = Open3.capture3(*command)
      abort "#{name} 无法解压：#{err}" unless status.success?
      entries = Dir.children(output)
      abort "#{name} 内容异常：#{entries.join(', ')}" unless entries == [entry]
      metadata = File.lstat(File.join(output, entry))
      abort "#{name} 里的程序不是普通文件" unless metadata.file?
      abort "#{name} 里的程序超过 128 MiB" if metadata.size > MAX_BYTES
      abort "#{name} 里的程序不可执行" if !name.end_with?('.zip') && (metadata.mode & 0o111).zero?
    end
    assets[name] = { 'name' => name, 'sha256' => digest }
    warn "#{name} #{digest}"
  end
  assets
end

def publish(version, dry_run: false)
  package = read_json(File.join(PACKAGE, 'package.json'))
  manifest = read_json(File.join(PACKAGE, 'install-manifest.json'))
  abort '版本不一致' unless [package['version'], manifest['version'], manifest['tag'].to_s.delete_prefix('v')].uniq == [version]
  abort '仓库不一致' unless manifest['repository'] == 'niuwoai/willdeep-cli'
  manifest['assets'] = verify_assets(manifest)
  # npm 10 先打包、再从磁盘重读 package.json 作为发给 registry 的元数据。
  # 打包那一步写进压缩包的 readme 不会出现在这份重读的清单里，registry 收到的
  # readme 就是空字符串，npmjs 页面因此显示没有 README。把 README 直接写进
  # package.json，重读时才带得走。
  readme = File.read(File.join(PACKAGE, 'README.md'))
  abort 'npm/willdeep/README.md 是空的' if readme.strip.empty?
  package['readme'] = readme
  package['readmeFilename'] = 'README.md'
  unless dry_run
    File.write(File.join(PACKAGE, 'install-manifest.json'), JSON.pretty_generate(manifest) + "\n")
    File.write(File.join(PACKAGE, 'package.json'), JSON.pretty_generate(package) + "\n")
  end

  if dry_run
    warn 'dry-run：已核验四个产物，不发布。'
    return
  end

  view_out, view_err, view_status = Open3.capture3('npm', 'view', "@niuwoai/willdeep@#{version}", 'version')
  if view_status.success? && view_out.strip == version
    warn "@niuwoai/willdeep@#{version} 已存在，跳过发布。"
    return
  end
  abort "无法确认 npm 版本：#{view_err}" unless view_err.include?('E404')

  _out, err, status = Open3.capture3('npm', 'publish', '--access', 'public', '--tag', 'rc', chdir: PACKAGE)
  abort "npm publish 失败：#{err}" unless status.success?
  warn "已发布 @niuwoai/willdeep@#{version}（tag rc）"
end

if $PROGRAM_NAME == __FILE__
  options = { dry_run: false }
  OptionParser.new do |parser|
    parser.on('--version VERSION') { |value| options[:version] = value }
    parser.on('--dry-run') { options[:dry_run] = true }
  end.parse!
  abort '缺少 --version' unless options[:version]
  publish(options[:version], dry_run: options[:dry_run])
end
