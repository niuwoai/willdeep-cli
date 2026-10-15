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
    MAXIMUM_BUDGET_MICROUSD = 20_000_000
    attr_reader :state

    def initialize(directory, manifest, expires_at: nil, resume_from_sha256: nil)
      FileUtils.mkdir_p(directory, mode: 0o700)
      @directory = directory
      @lock = File.open(File.join(directory, 'run.lock'), File::RDWR | File::CREAT, 0o600)
      raise Denied, 'experiment already running' unless @lock.flock(File::LOCK_EX | File::LOCK_NB)
      @rate = manifest.fetch('maximum_bill_microusd_per_million_tokens')
      @expected_route = manifest.fetch('expected_routed_model', 'fixed-route')
      @budget_cap = manifest.fetch('budget_cap_microusd', LIMITS[:microdollars])
      raise Denied, 'invalid effective bill rate' unless @rate.is_a?(Integer) && @rate.positive?
      raise Denied, 'invalid signed budget cap' unless @budget_cap.is_a?(Integer) && @budget_cap.positive? && @budget_cap <= MAXIMUM_BUDGET_MICROUSD
      raise Denied, 'wrong immutable proposal' unless manifest['proposal_sha256'] == PROPOSAL
      @mutex = Mutex.new
      @expires_at = expires_at
      if File.exist?(path)
        raise Denied, 'never overwrite or auto-resume an experiment ledger' unless resume_from_sha256
        resume_ledger(manifest, resume_from_sha256)
        return
      end
      raise Denied, 'resume requires the existing original ledger' if resume_from_sha256
      @state = {'manifest_sha256' => AgentRsiLive.digest(manifest), 'attempts' => [], 'requests' => [],
                'holdout_accesses' => 0, 'retries' => 0, 'holdout_retries' => 0, 'paused' => nil,
                'budget_cap_microusd' => @budget_cap}
      preserve_manifest(manifest)
      save
    rescue StandardError
      @lock.close if @lock && !@lock.closed?
      raise
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
      values = [row.fetch('accounted_microusd', row['reserved_microusd']), row.fetch('observed_bill_microusd', 0),
                row.fetch('verified_bill_microusd', 0)]
      values << row['reserved_microusd'] if %w[reserved unknown].include?(row['status'])
      values.compact.max || 0
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
        check!(exposure + cost_bound <= @budget_cap, 'global dollar reservation exhausted')
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

    # The Host verifies the signed grant and passes its exact raw-ledger digest.
    # This method never creates authority from ledger fields or a new budget.
    def resume_ledger(manifest, expected_raw_sha)
      check!(!@expires_at || Time.now < @expires_at, 'authorization expired')
      check!(expected_raw_sha.is_a?(String) && expected_raw_sha.match?(/\A[0-9a-f]{64}\z/), 'invalid resume ledger digest')
      metadata = File.lstat(path)
      check!(metadata.file? && !metadata.symlink?, 'resume ledger must be a regular file')
      raw = File.binread(path)
      check!(Digest::SHA256.hexdigest(raw) == expected_raw_sha, 'resume ledger differs from signed grant')
      JSON.parse(raw, object_class: Grader::UniqueObject)
      @state = JSON.parse(raw)
      validate_resume_state
      old_hash = @state.fetch('manifest_sha256')
      old_manifest = previous_manifest(old_hash)
      validate_resume_scope(old_manifest, manifest)
      new_hash = AgentRsiLive.digest(manifest)
      preserve_manifest(old_manifest)
      preserve_manifest(manifest)
      snapshot = File.join('amendments', "ledger-#{expected_raw_sha}.json")
      preserve_bytes(File.join(@directory, snapshot), raw)
      amendments = @state.fetch('amendments', [])
      check!(amendments.is_a?(Array), 'invalid existing amendment chain')
      amendment = {'resume_from_sha256' => expected_raw_sha, 'previous_manifest_sha256' => old_hash,
                   'manifest_sha256' => new_hash, 'ledger_snapshot' => snapshot,
                   'resume_receipt' => File.join('amendments', "resume-#{expected_raw_sha}.json"),
                   'previous_manifest_snapshot' => manifest_snapshot(old_hash),
                   'manifest_snapshot' => manifest_snapshot(new_hash),
                   'previous_amendment_sha256' => amendments.empty? ? nil : AgentRsiLive.digest(amendments.last),
                   'previous_budget_cap_microusd' => old_manifest.fetch('budget_cap_microusd', LIMITS[:microdollars]),
                   'budget_cap_microusd' => @budget_cap,
                   'previous_pause' => @state['paused'], 'amended_at' => Time.now.utc.iso8601}
      @state['amendments'] = amendments + [amendment]
      @state['manifest_sha256'] = new_hash
      @state['budget_cap_microusd'] = @budget_cap
      @state['paused'] = nil
      @active = nil
      save
      receipt = {'previous_ledger_sha256' => expected_raw_sha, 'amended_ledger_sha256' => Digest::SHA256.file(path).hexdigest,
                 'previous_manifest_sha256' => old_hash, 'manifest_sha256' => new_hash, 'amended_at' => amendment['amended_at']}
      preserve_bytes(File.join(@directory, amendment['resume_receipt']), AgentRsiLive.canonical(receipt))
    rescue JSON::ParserError, KeyError, TypeError => error
      raise Denied, "invalid resume ledger or manifest (#{error.class})"
    end

    def validate_resume_state
      check!(@state.is_a?(Hash) && @state['manifest_sha256'].is_a?(String) &&
             @state['manifest_sha256'].match?(/\A[0-9a-f]{64}\z/), 'invalid original manifest identity')
      attempts, requests = @state.values_at('attempts', 'requests')
      check!(attempts.is_a?(Array) && requests.is_a?(Array), 'invalid original ledger rows')
      check!(attempts.length <= LIMITS[:attempts] && requests.length <= LIMITS[:requests], 'original global limits exceeded')
      terminals = %w[passed quality_failure infrastructure_error incomplete budget_exhausted interrupted]
      check!(attempts.all? { |row| row.is_a?(Hash) && terminals.include?(row['status']) }, 'running or unresolved trial blocks resume')
      counters = {'holdout_accesses' => [:holdout_accesses, attempts.count { |r| r['split'] == 'holdout' }],
                  'retries' => [:retries, attempts.count { |r| r['attempt'] == 2 }],
                  'holdout_retries' => [:holdout_retries, attempts.count { |r| r['attempt'] == 2 && r['split'] == 'holdout' }]}
      counters.each do |name, (limit, minimum)|
        value = @state[name]
        check!(value.is_a?(Integer) && value >= minimum && value <= LIMITS[limit], 'invalid or reduced original quota counter')
      end
      requests.each do |row|
        check!(row.is_a?(Hash) && row['status'] == 'settled', 'reserved or unknown request blocks resume')
        %w[actual_tokens input_tokens output_tokens accounted_microusd verified_bill_microusd].each do |field|
          check!(row[field].is_a?(Integer) && row[field] >= 0, 'resume requires known settled tokens and verified bills')
        end
        check!(row['actual_tokens'] == row['input_tokens'] + row['output_tokens'], 'inconsistent settled token usage')
        if row.key?('observed_bill_microusd')
          check!(row['observed_bill_microusd'].is_a?(Integer) && row['observed_bill_microusd'] >= 0, 'invalid observed bill')
        end
      end
      check!(requests.sum { |row| row['actual_tokens'] } <= LIMITS[:tokens], 'original token cap exceeded')
      check!(requests.sum { |row| self.class.exposure(row) } <= @budget_cap, 'original dollar cap exceeded')
      check!(!@state['frozen_routed_model'] || @state['frozen_routed_model'] == @expected_route, 'existing frozen route differs from amendment')
    end

    def validate_resume_scope(previous, current)
      check!(previous['proposal_sha256'] == PROPOSAL && current['proposal_sha256'] == PROPOSAL, 'resume changed immutable proposal')
      scope = previous.dig('account', 'scope_sha256')
      check!(scope.is_a?(String) && !scope.empty? && current.dig('account', 'scope_sha256') == scope, 'resume changed account scope')
      original = File.realpath(@directory)
      check!([previous['run_directory'], current['run_directory']].all? { |directory| directory.is_a?(String) && File.realpath(directory) == original },
             'resume moved the original experiment directory')
    rescue Errno::ENOENT
      raise Denied, 'resume moved the original experiment directory'
    end

    def previous_manifest(hash)
      snapshot = File.join(@directory, manifest_snapshot(hash))
      source = File.exist?(snapshot) ? snapshot : File.join(@directory, 'manifest.json')
      check!(File.file?(source) && !File.symlink?(source), 'original manifest snapshot unavailable')
      manifest = JSON.parse(File.binread(source))
      check!(AgentRsiLive.digest(manifest) == hash, 'original manifest snapshot differs from ledger')
      manifest
    end

    def manifest_snapshot(hash)
      File.join('amendments', "manifest-#{hash}.json")
    end

    def preserve_manifest(manifest)
      hash = AgentRsiLive.digest(manifest)
      destination = File.join(@directory, manifest_snapshot(hash))
      if File.exist?(destination)
        check!(File.file?(destination) && !File.symlink?(destination) && AgentRsiLive.digest(JSON.parse(File.binread(destination))) == hash,
               'immutable manifest snapshot differs')
        return
      end
      source = File.join(@directory, 'manifest.json')
      raw = File.binread(source) if File.file?(source) && !File.symlink?(source)
      bytes = raw && AgentRsiLive.digest(JSON.parse(raw)) == hash ? raw : AgentRsiLive.canonical(manifest)
      preserve_bytes(destination, bytes)
    end

    def preserve_bytes(destination, bytes)
      FileUtils.mkdir_p(File.dirname(destination), mode: 0o700)
      if File.exist?(destination)
        check!(File.file?(destination) && !File.symlink?(destination) && File.binread(destination) == bytes, 'immutable amendment snapshot differs')
        return
      end
      File.open(destination, File::WRONLY | File::CREAT | File::EXCL, 0o600) do |file|
        file.write(bytes)
        file.flush
        file.fsync
      end
    end

    def check!(condition, reason)
      raise Denied, reason unless condition
    end
  end

  class Broker
    attr_reader :origin

    def initialize(ledger, upstream:, api_key:, directory:, transport: nil, expected_response_model: nil, expected_gateway_version: nil, billing_validator: nil)
      @ledger, @upstream, @api_key, @directory = ledger, URI(upstream), api_key, directory
      raise Denied, 'upstream must be fixed HTTPS inference endpoint' unless @upstream.scheme == 'https' && @upstream.path.end_with?('/chat/completions') && @upstream.userinfo.nil?
      @capability = @trial_id = nil
      @transport = transport
      @expected_response_model = expected_response_model
      @expected_gateway_version = expected_gateway_version
      @billing_validator = billing_validator
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
        raise Denied, 'gateway response model alias changed' if @expected_response_model && data['model'] != @expected_response_model
        raise Denied, 'gateway version changed' if @expected_gateway_version && headers['x-app-version'] != @expected_gateway_version
        @ledger.settle(record, data, headers)
        @billing_validator&.call(record)
        headers.each { |key, value| response[key] = value if value }
        response['X-RSI-Receipt-Id'] = record['receipt_id']
        response['X-RSI-Approved-Model'] = MODEL
        response.status, response.body = 200, result.body
      rescue StandardError => error
        reason = error.is_a?(Denied) ? error.message : 'network, bill or malformed response unresolved'
        if record['status'] == 'settled' && record.key?('observed_bill_microusd')
          record['execution_status'] = 'rejected_at_billing_gate'
          @ledger.pause(reason)
        elsif record['status'] != 'unknown'
          @ledger.unknown(record, reason)
        end
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

end

require_relative 'agent_rsi_grader'
