#!/usr/bin/env ruby
# frozen_string_literal: true

# 发布脚本的重定向判断。不下载 GitHub、不执行 npm publish。
#
#   ruby scripts/test/publish_npm_willdeep_test.rb

require 'minitest/autorun'
require 'uri'

load File.expand_path('../publish_npm_willdeep.rb', __dir__)

class PublishNpmWilldeepTest < Minitest::Test
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
end
