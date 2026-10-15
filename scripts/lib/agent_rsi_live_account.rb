# frozen_string_literal: true
require 'bigdecimal'
require_relative 'agent_rsi_live'
require_relative 'willdeep_credentials'

module AgentRsiLive
  # Metadata calls only. Never save session JSON or the raw key-list response.
  class Account
    attr_reader :base

    def initialize(home = File.expand_path('~/.willdeep'))
      provider, @base, @key = WilldeepCredentials.default_provider(File.join(home, 'config.toml'))
      @session = JSON.parse(File.read(File.join(home, 'agent-session.json')))
      endpoint = URI(@base)
      raise Denied, 'unexpected inference provider/origin' unless provider == 'some-im' && endpoint.host == 'api.niuwoai.com' &&
        endpoint.scheme == 'https' && endpoint.path.sub(%r{/+$}, '') == '/v1' && endpoint.userinfo.nil? && endpoint.query.nil? && endpoint.fragment.nil?
      raise Denied, 'unexpected customer origin' unless @session['origin'] == 'https://some.im'
    end

    def broker_key
      @key
    end

    def get(url, token)
      uri = URI(url)
      http = Net::HTTP.new(uri.host, uri.port, nil)
      http.use_ssl, http.max_retries = true, 0
      http.open_timeout, http.read_timeout = 10, 25
      request = Net::HTTP::Get.new(uri)
      request['Authorization'] = "Bearer #{token}"
      response = http.request(request)
      raise Denied, "metadata HTTP #{response.code}" unless response.code == '200'
      result = JSON.parse(response.body)
      raise Denied, 'customer metadata rejected' if result.key?('code') && result['code'] != 0
      result
    rescue Denied
      raise
    rescue StandardError
      raise Denied, 'account metadata unavailable; no inference permitted'
    end

    def customer(path)
      get(@session.fetch('origin') + '/api/v1/customer/' + path, @session.fetch('token')).fetch('data')
    end

    def snapshot(check_key_policy: true)
      balance = get(@base.sub(%r{/+$}, '') + '/account/balance', @key)
      tenant = customer('tenant').fetch('tenant')
      raise Denied, 'inference and customer identity differ' unless balance.fetch('tenant_id') == @session.fetch('tenant_id') && tenant.fetch('id') == balance['tenant_id']
      raise Denied, 'insufficient USD balance' unless balance['currency'] == 'USD' && BigDecimal(balance.fetch('balance')) >= 5
      model = customer('models-catalog').fetch('text_items').find { |row| row['request_model_name'] == MODEL }
      raise Denied, 'account price missing' unless model && model['bill_input_per_1m'] && model['bill_output_per_1m']
      multiplier = BigDecimal(tenant.fetch('billing_multiplier'))
      input = BigDecimal(model.fetch('bill_input_per_1m')) * multiplier
      output = BigDecimal(model.fetch('bill_output_per_1m')) * multiplier
      raise Denied, 'invalid account price' unless input >= 0 && output.positive?
      result = {'observed_at' => Time.now.utc.iso8601, 'same_account_verified' => true,
                'scope_sha256' => Digest::SHA256.hexdigest(balance.fetch('tenant_id')),
                'currency' => 'USD', 'balance_sufficient_for_5_usd' => true,
                'model' => MODEL, 'bill_input_per_1m' => input.to_s('F'), 'bill_output_per_1m' => output.to_s('F'),
                'cost_margin' => model.fetch('cost_margin'), 'billing_multiplier' => multiplier.to_s('F'),
                'maximum_bill_microusd_per_million_tokens' => ([input, output].max * 1_000_000).ceil}
      if check_key_policy
        payload = customer('api-keys?page_size=200')
        matches = []
        visit = lambda do |value|
          if value.is_a?(Hash)
            prefix = value['key_prefix']
            matches << value if prefix.is_a?(String) && !prefix.empty? && @key.start_with?(prefix)
            value.each_value { |child| visit.call(child) }
          elsif value.is_a?(Array)
            value.each { |child| visit.call(child) }
          end
        end
        visit.call(payload)
        raise Denied, 'cannot verify exact inference key policy' unless matches.length == 1
        policy = matches.first
        raise Denied, 'inference key inactive or permits model fallback' unless policy['status'] == 'active' && policy['enable_failover'] == false
        result['key_model_fallback_disabled'] = true
      end
      result
    end

    def billing_receipts(trial_id)
      data = customer('usage/logs?view=request&page_size=20&include_summary=false&session_id=' + URI.encode_www_form_component(trial_id))
      # Support only the documented request-list envelope; unknown shape stops
      # reconciliation instead of interpreting it as an empty/free bill.
      rows = data.is_a?(Array) ? data : (data['items'] || data['rows'])
      raise Denied, 'unrecognized billing receipt envelope' unless rows.is_a?(Array)
      rows.map { |row| row.slice('request_id', 'model', 'input_tokens', 'output_tokens', 'cache_read_tokens', 'cache_write_tokens',
                               'total_cost', 'billing_multiplier', 'cost', 'failover_from', 'stream_status') }
    end
  end
end
