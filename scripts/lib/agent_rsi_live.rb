# frozen_string_literal: true

# Local, explicitly approved experiment support. This is not the mock-only RSI
# service contract and does not upload evidence or change an installed agent.
require 'json'
require 'digest'
require 'securerandom'
require 'fileutils'
require 'time'
require 'net/http'
require 'webrick'
require_relative 'agent_rsi_contract'

module AgentRsiLive
  MODEL = 'deepseek-v4-flash'
  PROPOSAL = 'a3ae36e96d268f8aaab8a02efacea61cb68f58c1d3bf812b9a38311890736606'
  SUITE = '281f7dadf99f4cf86c4de5a922cdac7531c933654f878bb6b8f532d502a96575'
  REVISIONS = {
    'baseline' => '51b79a923ceceeae80a7cc6e20f2f0dbdfdb136f1bc516b895450ea633273bdd',
    'candidate' => '9554ab2e7fb5449750c903716efdbb640215af6fe34975609b17fd3d39a79306'
  }.freeze
  LIMITS = {attempts: 360, requests: 1440, tokens: 5_760_000, microdollars: 5_000_000,
            trial_requests: 4, trial_tokens: 16_000, output_tokens: 4000,
            retries: 72, holdout_retries: 18, holdout_accesses: 90}.freeze
  # No tokenizer endpoint is available. UTF-8 bytes bound byte-pair tokenized
  # request content conservatively; add framing allowance for message/tool
  # delimiters. Any actual overrun stops the batch rather than relaxing limits.
  FRAMING_ALLOWANCE = 1024

  class Denied < StandardError; end

  def self.canonical(value)
    ContractGolden.canonical(value)
  end

  def self.digest(value)
    Digest::SHA256.hexdigest(canonical(value))
  end

  def self.write(path, value)
    bytes = value.is_a?(String) ? value : JSON.pretty_generate(value) + "\n"
    temporary = path + '.new'
    File.open(temporary, File::WRONLY | File::CREAT | File::EXCL, 0o600) do |file|
      file.write(bytes)
      file.flush
      file.fsync
    end
    File.rename(temporary, path)
  end

  # One root-owned ledger, one process lock, one active trial, and no automatic
  # replay of requests with missing receipts. Reservation is persisted BEFORE
  # dispatch, including holdout accesses and all attempts.
  class Ledger
    attr_reader :state

    def initialize(directory, manifest, expires_at: nil)
      FileUtils.mkdir_p(directory, mode: 0o700)
      @directory = directory
      @lock = File.open(File.join(directory, 'run.lock'), File::RDWR | File::CREAT, 0o600)
      raise Denied, 'experiment already running' unless @lock.flock(File::LOCK_EX | File::LOCK_NB)
      raise Denied, 'never overwrite or auto-resume an experiment ledger' if File.exist?(path)
      @rate = manifest.fetch('maximum_bill_microusd_per_million_tokens')
      @expected_route = manifest.fetch('expected_routed_model', 'fixed-route')
      raise Denied, 'invalid effective bill rate' unless @rate.is_a?(Integer) && @rate.positive?
      raise Denied, 'wrong immutable proposal' unless manifest['proposal_sha256'] == PROPOSAL
      @mutex = Mutex.new
      @expires_at = expires_at
      @state = {'manifest_sha256' => AgentRsiLive.digest(manifest), 'attempts' => [], 'requests' => [],
                'holdout_accesses' => 0, 'retries' => 0, 'holdout_retries' => 0, 'paused' => nil}
      save
    end

    def close
      @lock.close
    end

    def path
      File.join(@directory, 'ledger.json')
    end

    def save
      AgentRsiLive.write(path, @state)
    end

    def pause(reason)
      @state['paused'] ||= reason
      save
    end

    def self.exposure(row)
      [row.fetch('accounted_microusd', row['reserved_microusd']), row.fetch('observed_bill_microusd', 0)].max
    end

    def begin_trial(trial)
      @mutex.synchronize do
        check!(!@state['paused'], 'experiment paused')
        check!(!@expires_at || Time.now < @expires_at, 'authorization expired')
        check!(@active.nil?, 'another trial active')
        check!(@state['attempts'].length < LIMITS[:attempts], 'attempt limit')
        check!(REVISIONS[trial['arm']] == trial['agent_revision'], 'unapproved agent revision')
        check!(!@state['attempts'].any? { |row| row['trial_id'] == trial['trial_id'] }, 'duplicate trial id')
        if trial.fetch('attempt') == 2
          parent = @state['attempts'].find { |row| row['trial_id'] == trial['resume_of'] }
          check!(parent && parent['status'] == 'infrastructure_error' && parent['attempt'] == 1, 'ineligible retry')
          %w[platform task_id arm repetition split agent_revision].each { |key| check!(parent[key] == trial[key], 'retry changed identity') }
          check!(!@state['attempts'].any? { |row| row['resume_of'] == parent['trial_id'] }, 'retry already consumed')
          check!(@state['retries'] < LIMITS[:retries], 'retry limit')
          check!(@state['holdout_retries'] < LIMITS[:holdout_retries], 'holdout retry limit') if trial['split'] == 'holdout'
          @state['retries'] += 1
          @state['holdout_retries'] += 1 if trial['split'] == 'holdout'
        else
          check!(trial['attempt'] == 1 && trial['resume_of'].nil?, 'invalid attempt')
          check!(!@state['attempts'].any? { |row| %w[platform task_id arm repetition].all? { |key| row[key] == trial[key] } }, 'duplicate planned trial')
        end
        if trial['split'] == 'holdout'
          check!(@state['holdout_accesses'] < LIMITS[:holdout_accesses], 'holdout access limit')
          @state['holdout_accesses'] += 1
        end
        @active = trial.merge('status' => 'running', 'started_at' => Time.now.utc.iso8601)
        @state['attempts'] << @active
        save
      end
    end

    def finish_trial(status)
      @mutex.synchronize do
        check!(!@active.nil?, 'no active trial')
        @active['status'] = status
        @active['finished_at'] = Time.now.utc.iso8601
        @active = nil
        save
      end
    end

    def authorize_request(trial_id, sequence, body)
      @mutex.synchronize do
        check!(!@state['paused'], 'experiment paused')
        check!(!@expires_at || Time.now < @expires_at, 'authorization expired')
        check!(@active && @active['trial_id'] == trial_id, 'inactive trial')
        check!(body['model'] == MODEL && body['stream'] == false && body['temperature'] == 0, 'unapproved model or parameters')
        check!((body.keys - %w[model stream temperature max_tokens messages tools tool_choice parallel_tool_calls]).empty?, 'unapproved request fields')
        check!(body.fetch('messages').is_a?(Array), 'missing messages')
        check!(!body.key?('max_completion_tokens') && !body.key?('n') && !body.key?('reasoning_effort'), 'unapproved output parameters')
        rows = @state['requests'].select { |row| row['trial_id'] == trial_id }
        check!(rows.length < LIMITS[:trial_requests] && sequence == rows.length + 1, 'trial request limit or repeated request')
        check!(rows.none? { |row| row['status'] == 'reserved' || row['status'] == 'unknown' }, 'unsettled request')
        check!(@state['requests'].length < LIMITS[:requests], 'global request limit')
        input_bound = JSON.generate(body.slice('messages', 'tools', 'tool_choice')).bytesize + FRAMING_ALLOWANCE
        trial_used = rows.sum { |row| row.fetch('actual_tokens', row['reserved_tokens']) }
        global_used = @state['requests'].sum { |row| row.fetch('actual_tokens', row['reserved_tokens']) }
        output_cap = [Integer(body.fetch('max_tokens')), LIMITS[:output_tokens],
                      LIMITS[:trial_tokens] - trial_used - input_bound,
                      LIMITS[:tokens] - global_used - input_bound].min
        check!(output_cap.positive?, 'conservative token reservation exhausted')
        reserved = input_bound + output_cap
        cost_bound = (reserved * @rate + 999_999) / 1_000_000
        exposure = @state['requests'].sum { |row| self.class.exposure(row) }
        check!(exposure + cost_bound <= LIMITS[:microdollars], 'global dollar reservation exhausted')
        admitted = body.merge('max_tokens' => output_cap)
        record = {'receipt_id' => SecureRandom.uuid, 'trial_id' => trial_id, 'sequence' => sequence,
                  'status' => 'reserved', 'reserved_input_tokens' => input_bound, 'max_tokens' => output_cap,
                  'reserved_tokens' => reserved, 'reserved_microusd' => cost_bound,
                  'requested_body_sha256' => AgentRsiLive.digest(body),
                  'request_sha256' => AgentRsiLive.digest(admitted), 'started_at' => Time.now.utc.iso8601}
        @state['requests'] << record
        save
        [record, admitted]
      end
    end

    def settle(record, response, headers)
      @mutex.synchronize do
        usage = response.fetch('usage')
        input = usage.fetch('prompt_tokens')
        output = usage.fetch('completion_tokens')
        total = usage.fetch('total_tokens')
        check!([input, output, total].all? { |value| value.is_a?(Integer) && value >= 0 }, 'unknown token usage')
        check!(total == input + output && input <= record['reserved_input_tokens'] && output <= record['max_tokens'], 'token reservation exceeded')
        check!(!headers['x-request-id'].to_s.empty? && !headers['x-routed-model'].to_s.empty?, 'missing provider request or route evidence')
        check!(@state['requests'].none? { |row| row != record && row['request_id'] == headers['x-request-id'] }, 'provider request id was reused')
        route = headers['x-routed-model']
        check!(route == @expected_route, 'provider route differs from frozen mapping')
        @state['frozen_routed_model'] ||= route
        check!(@state['frozen_routed_model'] == route, 'provider route changed')
        check!(response['id'] && response['id'] != 'chatcmpl-stub', 'missing or stub response id')
        # Conservatively account ALL token classes at the highest actual account
        # rate, including cache hits and reasoning. Billing reconciliation may
        # lower a report, never grant more execution than this local ledger.
        record.merge!('status' => 'settled', 'actual_tokens' => total,
                      'input_tokens' => input, 'output_tokens' => output,
                      'accounted_microusd' => (total * @rate + 999_999) / 1_000_000,
                      'request_id' => headers['x-request-id'], 'routed_model' => route,
                      'response_id' => response['id'], 'response_sha256' => AgentRsiLive.digest(response),
                      'finished_at' => Time.now.utc.iso8601)
        save
      end
    rescue StandardError => error
      unknown(record, error.is_a?(Denied) ? error.message : 'missing or invalid usage')
      raise Denied, @state['paused']
    end

    def unknown(record, reason)
      record['status'] = 'unknown'
      record['error'] = reason
      pause(reason)
    end

    private

    def check!(condition, reason)
      raise Denied, reason unless condition
    end
  end

  class Broker
    attr_reader :origin

    def initialize(ledger, upstream:, api_key:, directory:, transport: nil)
      @ledger, @upstream, @api_key, @directory = ledger, URI(upstream), api_key, directory
      raise Denied, 'upstream must be fixed HTTPS inference endpoint' unless @upstream.scheme == 'https' && @upstream.path.end_with?('/chat/completions') && @upstream.userinfo.nil?
      @capability = @trial_id = nil
      @transport = transport
      @server = WEBrick::HTTPServer.new(BindAddress: '127.0.0.1', Port: 0, AccessLog: [], Logger: WEBrick::Log.new(File::NULL), RequestTimeout: 125)
      @origin = "http://127.0.0.1:#{@server.config[:Port]}"
      @server.mount_proc('/v1/chat/completions') { |request, response| handle(request, response) }
      @thread = Thread.new { @server.start }
    end

    def activate(trial_id)
      @trial_id, @capability = trial_id, SecureRandom.hex(32)
      @capability
    end

    def deactivate
      @trial_id = @capability = nil
    end

    def close
      @server.shutdown
      @thread.join
    end

    def handle(request, response)
      response['Content-Type'] = 'application/json'
      unless request.request_method == 'POST' && request.path == '/v1/chat/completions' && @capability &&
             request['authorization'] == "Bearer #{@capability}" && request['x-rsi-trial-id'] == @trial_id && request['x-rsi-proposal-sha256'] == PROPOSAL
        raise Denied, 'missing or stale trial admission'
      end
      raise Denied, 'oversize request' if request.body.to_s.bytesize > 100_000
      body = JSON.parse(request.body)
      record, admitted = @ledger.authorize_request(@trial_id, Integer(request['x-rsi-request-sequence']), body)
      sequence = @ledger.state['requests'].length
      AgentRsiLive.write(File.join(@directory, format('request-%04d.json', sequence)), admitted)
      begin
        outbound = Net::HTTP::Post.new(@upstream)
        outbound['Authorization'] = "Bearer #{@api_key}"
        outbound['Content-Type'] = 'application/json'
        outbound['X-Client-Name'] = 'willdeep-rsi-approved-pilot'
        outbound['User-Agent'] = 'WillDeep-RSI-approved-pilot/1'
        outbound['X-Playground-Session-ID'] = @trial_id
        outbound.body = JSON.generate(admitted)
        result = @transport ? @transport.call(outbound) : dispatch(outbound)
        headers = %w[x-request-id x-routed-model x-app-version].to_h { |key| [key, result[key]] }
        AgentRsiLive.write(File.join(@directory, format('response-%04d.json', sequence)),
                           {'http_status' => result.code.to_i, 'headers' => headers, 'body' => result.body})
        raise Denied, "upstream HTTP #{result.code}; charge unresolved" unless result.code == '200'
        data = JSON.parse(result.body)
        @ledger.settle(record, data, headers)
        headers.each { |key, value| response[key] = value if value }
        response['X-RSI-Receipt-Id'] = record['receipt_id']
        response['X-RSI-Approved-Model'] = MODEL
        response.status, response.body = 200, result.body
      rescue StandardError => error
        @ledger.unknown(record, error.is_a?(Denied) ? error.message : 'network or malformed response; charge unresolved') unless record['status'] == 'unknown'
        raise Denied, @ledger.state['paused']
      end
    rescue StandardError => error
      response.status = 403
      response.body = JSON.generate(error: {type: 'rsi_admission_denied', message: error.is_a?(Denied) ? error.message : 'invalid broker request'})
    end

    def dispatch(outbound)
      http = Net::HTTP.new(@upstream.host, @upstream.port, nil)
      http.use_ssl = true
      http.open_timeout, http.read_timeout, http.write_timeout = 10, 110, 10
      http.max_retries = 0
      http.request(outbound)
    end
  end

  module Grader
    module_function

    class UniqueObject < Hash
      def []=(key, value)
        raise JSON::ParserError, 'duplicate object key' if key?(key)
        super
      end
    end

    # Parse top-level JSON objects while respecting strings and escapes. Do not
    # grade Worker wrappers or accept several conflicting model answers.
    def objects(text)
      objects, start, depth, quoted, escaped = [], nil, 0, false, false
      text.each_char.with_index do |character, index|
        if quoted
          if escaped then escaped = false
          elsif character == '\\' then escaped = true
          elsif character == '"' then quoted = false
          end
        elsif character == '"' && depth.positive? then quoted = true
        elsif character == '{'
          start = index if depth.zero?
          depth += 1
        elsif character == '}' && depth.positive?
          depth -= 1
          objects << text[start..index] if depth.zero?
        end
      end
      return [] unless depth.zero?
      objects.map { |object| JSON.parse(object, object_class: UniqueObject, allow_duplicate_key: false) rescue nil }
    end

    def grade(raw, expected, read_observed:, fixture_unchanged:)
      answers = objects(raw)
      answer = answers.length == 1 ? answers.first : nil
      valid = answer.is_a?(Hash) && answer.keys.sort == %w[has_defect line reason] &&
              [true, false].include?(answer['has_defect']) && answer['reason'].is_a?(String) && !answer['reason'].strip.empty? &&
              (answer['line'].nil? || (answer['line'].is_a?(Integer) && answer['line'].positive?)) &&
              (answer['has_defect'] ? !answer['line'].nil? : answer['line'].nil?)
      classification = valid && answer['has_defect'] == expected['has_defect']
      localization = classification && answer['line'] == expected['line']
      {'schema' => 'willdeep.agent-rsi.host-verifier.v1', 'schema_valid' => !!valid,
       'classification_correct' => !!classification, 'localization_correct' => !!localization,
       'read_observed' => read_observed == true, 'fixture_unchanged' => fixture_unchanged == true,
       'passed' => !!(localization && read_observed && fixture_unchanged),
       'raw_model_sha256' => Digest::SHA256.hexdigest(raw), 'oracle_sha256' => AgentRsiLive.digest(expected),
       'claim_limit' => 'No semantic reason/reproduction grading; exact seeded line is auxiliary.'}
    end
  end
end
