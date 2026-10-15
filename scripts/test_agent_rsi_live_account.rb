#!/usr/bin/env ruby
# frozen_string_literal: true
require 'minitest/autorun'
require_relative 'lib/agent_rsi_live_account'

class AgentRsiLiveAccountTest < Minitest::Test
  class Fixture < AgentRsiLive::Account
    attr_reader :calls, :delays
    def initialize(values)
      @values, @calls, @delays = values, 0, []
    end
    def get_once(_url, _token)
      @calls += 1
      value = @values.shift
      raise value if value.is_a?(Exception)
      value
    end
    def sleep(seconds) = @delays.push(seconds)
  end

  def test_transient_read_retries_once
    account = Fixture.new([Net::ReadTimeout.new, {'ok' => true}])
    assert_equal({'ok' => true}, account.get('unused', 'unused'))
    assert_equal 2, account.calls
    assert_equal [1], account.delays
  end

  def test_transient_http_retries_once
    account = Fixture.new([AgentRsiLive::Account::TransientMetadataFailure.new, {}])
    assert_equal({}, account.get('unused', 'unused'))
    assert_equal 2, account.calls
    assert_equal %w[429 502 503 504], AgentRsiLive::Account::TRANSIENT_HTTP_CODES
  end

  def test_transient_retry_is_bounded
    account = Fixture.new([EOFError.new, EOFError.new, {}])
    assert_raises(AgentRsiLive::Denied) { account.get('unused', 'unused') }
    assert_equal 2, account.calls
    assert_equal [1], account.delays
  end

  def test_authentication_rejection_is_not_retried
    account = Fixture.new([AgentRsiLive::Denied.new('metadata HTTP 401'), {}])
    assert_raises(AgentRsiLive::Denied) { account.get('unused', 'unused') }
    assert_equal 1, account.calls
    assert_empty account.delays
  end

  def test_invalid_metadata_is_not_retried
    account = Fixture.new([AgentRsiLive::Denied.new('customer metadata rejected'), {}])
    assert_raises(AgentRsiLive::Denied) { account.get('unused', 'unused') }
    assert_equal 1, account.calls
    assert_empty account.delays
  end

  def test_pricing_rejection_is_not_retried
    account = Fixture.new([AgentRsiLive::Denied.new('invalid account price'), {}])
    assert_raises(AgentRsiLive::Denied) { account.get('unused', 'unused') }
    assert_equal 1, account.calls
  end
end
