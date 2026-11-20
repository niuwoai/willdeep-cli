#!/usr/bin/env ruby
# frozen_string_literal: true

# 发布脚本的离线契约测试：重定向、发布顺序、标签同步与失败重试。
# 文件、产物核验和外部命令均使用 mock，不下载 GitHub、不执行真实 npm 命令。
#
#   ruby scripts/test/publish_npm_willdeep_test.rb

require 'minitest/autorun'
require 'minitest/mock'
require 'uri'

load File.expand_path('../publish_npm_willdeep.rb', __dir__)

class PublishNpmWilldeepTest < Minitest::Test
  VERSION = '0.123.0-rc2'
  PACKAGE_SPEC = "@niuwoai/willdeep@#{VERSION}"
  README = "# willdeep\n\nCLI package.\n"
  CommandStatus = Struct.new(:exitstatus) do
    def success?
      exitstatus.zero?
    end
  end

  def setup
    @assets = ASSETS.to_h do |name, _entry|
      [name, { 'name' => name, 'sha256' => 'a' * 64 }]
    end
    @files = {
      package_file('package.json') => JSON.generate('name' => '@niuwoai/willdeep', 'version' => VERSION),
      package_file('install-manifest.json') => JSON.generate(
        'version' => VERSION, 'tag' => "v#{VERSION}", 'repository' => 'niuwoai/willdeep-cli', 'assets' => {}
      ),
      package_file('README.md') => README
    }
    @verified_assets = 0
  end

  def test_https_default_port_on_a_trusted_host_is_allowed
    target = URI('https://release-assets.githubusercontent.com/github-production-release-asset/file')
    assert_equal 443, target.port
    assert trusted_redirect?(target)
    assert trusted_redirect?(URI('https://github.com/niuwoai/willdeep-cli/releases/download/v1/file'))
  end

  def test_explicit_port_and_untrusted_host_are_refused
    refute trusted_redirect?(URI('https://github.com:8443/unrelated'))
    refute trusted_redirect?(URI('https://example.test/file'))
    refute trusted_redirect?(URI('http://github.com/file'))
  end

  def test_new_version_is_published_before_rc_and_latest_are_updated
    with_publication([missing_version, tags_response, success, success, success]) { publish(VERSION) }

    assert_equal [view_call, tags_view_call, publish_call, tag_call('rc'), tag_call('latest')], @npm_calls
    assert_equal 1, @verified_assets
    assert_equal [package_file('install-manifest.json'), package_file('package.json')], @writes
    assert_equal @assets, JSON.parse(@files.fetch(package_file('install-manifest.json')))['assets']
    assert_equal README, JSON.parse(@files.fetch(package_file('package.json')))['readme']
  end

  def test_existing_version_repairs_both_tags_without_publishing_again
    with_publication([existing_version, tags_response, success, success]) { publish(VERSION) }

    assert_equal [view_call, tags_view_call, tag_call('rc'), tag_call('latest')], @npm_calls
    assert_equal 1, @verified_assets
  end

  def test_publish_failure_does_not_move_any_tags
    _out, err = with_publication([missing_version, tags_response, failure('publish denied')]) do
      error = assert_raises(SystemExit) { publish(VERSION) }
      refute error.success?
    end

    assert_equal [view_call, tags_view_call, publish_call], @npm_calls
    assert_includes err, 'npm publish 失败'
    assert_includes err, 'publish denied'
  end

  def test_rc_tag_failure_stops_before_latest_and_reports_the_tag
    _out, err = with_publication([existing_version, tags_response, failure('rc denied')]) do
      error = assert_raises(SystemExit) { publish(VERSION) }
      refute error.success?
    end

    assert_equal [view_call, tags_view_call, tag_call('rc')], @npm_calls
    assert_includes err, 'npm dist-tag rc 同步失败'
    assert_includes err, PACKAGE_SPEC
    assert_includes err, 'rc denied'
  end

  def test_latest_tag_failure_is_repaired_by_retrying_the_published_version
    _out, err = with_publication([missing_version, tags_response, success, success, failure('latest denied')]) do
      error = assert_raises(SystemExit) { publish(VERSION) }
      refute error.success?
    end

    assert_equal [view_call, tags_view_call, publish_call, tag_call('rc'), tag_call('latest')], @npm_calls
    assert_includes err, 'npm dist-tag latest 同步失败'
    assert_includes err, PACKAGE_SPEC
    assert_includes err, 'latest denied'

    with_publication([existing_version, tags_response('rc' => VERSION), success, success]) { publish(VERSION) }

    assert_equal [view_call, tags_view_call, tag_call('rc'), tag_call('latest')], @npm_calls
  end

  def test_old_existing_version_does_not_replace_newer_tags
    _out, err = with_publication([existing_version, tags_response('rc' => '0.123.0-rc3', 'latest' => '0.123.0-rc3')]) do
      publish(VERSION)
    end
    assert_equal [view_call, tags_view_call], @npm_calls
    assert_includes err, '已过期'
  end

  def test_old_unpublished_version_does_not_publish_with_the_rc_tag
    with_publication([missing_version, tags_response('latest' => '0.123.0-rc3')]) { publish(VERSION) }
    assert_equal [view_call, tags_view_call], @npm_calls
  end

  def test_rc_sequence_is_numeric_and_stable_release_is_newer
    assert_operator npm_release_key('0.123.0-rc10') <=> npm_release_key('0.123.0-rc2'), :>, 0
    assert_operator npm_release_key('0.123.0') <=> npm_release_key('0.123.0-rc10'), :>, 0
    assert_operator npm_release_key('0.124.0-rc1') <=> npm_release_key('0.123.0'), :>, 0
  end

  def test_unknown_tag_version_blocks_updates
    _out, err = with_publication([existing_version, tags_response('latest' => 'not-a-version')]) do
      error = assert_raises(SystemExit) { publish(VERSION) }
      refute error.success?
    end
    assert_equal [view_call, tags_view_call], @npm_calls
    assert_includes err, '无法比较 npm 发行版本'
  end

  def test_tag_lookup_failure_blocks_publication
    _out, err = with_publication([missing_version, failure('registry unavailable')]) do
      error = assert_raises(SystemExit) { publish(VERSION) }
      refute error.success?
    end
    assert_equal [view_call, tags_view_call], @npm_calls
    assert_includes err, '无法确认 npm 标签'
  end

  def test_invalid_tag_response_blocks_publication
    ['[]', '{'].each do |response|
      with_publication([missing_version, success(response)]) do
        error = assert_raises(SystemExit) { publish(VERSION) }
        refute error.success?
      end
      assert_equal [view_call, tags_view_call], @npm_calls
    end
  end

  def test_dry_run_verifies_assets_without_writes_or_npm_commands
    original_files = @files.dup
    with_publication([]) { publish(VERSION, dry_run: true) }

    assert_equal 1, @verified_assets
    assert_equal original_files, @files
    assert_empty @writes
    assert_empty @npm_calls
  end

  private

  def package_file(name)
    File.join(PACKAGE, name)
  end

  def success(output = '')
    [output, '', CommandStatus.new(0)]
  end

  def failure(message)
    ['', message, CommandStatus.new(1)]
  end

  def missing_version
    failure('npm ERR! code E404')
  end

  def existing_version
    success("#{VERSION}\n")
  end

  def tags_response(tags = {})
    success(JSON.generate(tags))
  end

  def tags_view_call
    [['npm', 'view', '@niuwoai/willdeep', 'dist-tags', '--json'], {}]
  end

  def view_call
    [['npm', 'view', PACKAGE_SPEC, 'version'], {}]
  end

  def publish_call
    [['npm', 'publish', '--access', 'public', '--tag', 'rc'], { chdir: PACKAGE }]
  end

  def tag_call(tag)
    [['npm', 'dist-tag', 'add', PACKAGE_SPEC, tag], {}]
  end

  def with_publication(responses)
    @npm_calls = []
    @writes = []
    read_file = ->(path) { @files.fetch(path).dup }
    write_file = lambda do |path, contents|
      @writes << path
      @files[path] = contents
      contents.bytesize
    end
    verify = lambda do |_manifest|
      @verified_assets += 1
      @assets
    end
    run_command = lambda do |*command, **options|
      @npm_calls << [command, options]
      flunk "意外的外部命令：#{command.inspect}" if responses.empty?
      responses.shift
    end
    deny_http = ->(*) { flunk '离线测试禁止访问 HTTP' }

    output = capture_io do
      File.stub(:read, read_file) do
        File.stub(:write, write_file) do
          stub(:verify_assets, verify) do
            Net::HTTP.stub(:start, deny_http) do
              Open3.stub(:capture3, run_command) { yield }
            end
          end
        end
      end
    end
    assert_empty responses, '预期的命令未全部执行'
    output
  end
end
