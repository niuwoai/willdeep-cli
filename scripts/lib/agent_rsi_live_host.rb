# frozen_string_literal: true
require 'open3'
require 'openssl'
require 'timeout'
require_relative 'agent_rsi_live_account'

module AgentRsiLive
  class Host
    ROOT = File.expand_path('../..', __dir__)
    SOURCE_FILES = %w[scripts/lib/agent_rsi_live.rb scripts/lib/agent_rsi_grader.rb scripts/lib/agent_rsi_live_account.rb scripts/lib/agent_rsi_live_host.rb
                      scripts/lib/agent_rsi_contract.rb scripts/lib/willdeep_credentials.rb
                      docs/decisions/2026-10-15-agent-rsi-live-preregistration.md].freeze
    QUALITY_TERMINALS = %w[passed quality_failure].freeze
    KNOWN_TERMINALS = (QUALITY_TERMINALS + ['budget_exhausted']).freeze

    def recovery_v2?
      @manifest['protocol_revision'] == 'v2-known-terminal'
    end

    # The frozen Mac schema has no usage_complete flag. Derive completeness
    # from every native receipt and the broker totals, never a default true.
    def complete_usage?(observation, receipts)
      return false if observation.key?('usage_complete') && observation['usage_complete'] != true
      return false if receipts.empty?
      fields = %w[input_tokens output_tokens actual_tokens]
      return false unless receipts.all? { |row| fields.all? { |key| row[key].is_a?(Integer) && row[key] >= 0 } && row['input_tokens'] + row['output_tokens'] == row['actual_tokens'] }
      native = observation['request_receipts']
      return false unless native.is_a?(Array) && native.length == receipts.length && native.map { |row| row['receipt_id'] }.uniq.length == native.length
      return false unless native.all? do |row|
        root = receipts.find { |item| item['receipt_id'] == row['receipt_id'] }
        root && row.dig('usage', 'prompt_tokens') == root['input_tokens'] &&
          row.dig('usage', 'completion_tokens') == root['output_tokens'] && row.dig('usage', 'total_tokens') == root['actual_tokens']
      end
      observation['input_tokens'] == receipts.sum { |row| row['input_tokens'] } &&
        observation['output_tokens'] == receipts.sum { |row| row['output_tokens'] } &&
        (!observation.key?('total_tokens') || observation['total_tokens'] == receipts.sum { |row| row['actual_tokens'] })
    end

    def budget_terminal?(trial, observation, receipts)
      return true if observation['did_truncate'] == true
      if trial['platform'] == 'rust-native-worker'
        return true if observation.dig('runtime', 'adapter') == 'rust-worker-native-broker-v1' &&
          observation['worker_status'] == 'budget_exhausted' && complete_usage?(observation, receipts)
        return true unless budget_validation_view(trial, observation, receipts).equal?(observation)
      end
      return false unless trial['platform'] == 'macos-native-worker' &&
        observation.dig('runtime', 'adapter') == 'macos-native-worker.v1' &&
        observation['raw_model_content'].nil? && receipts.length == LIMITS[:trial_requests] &&
        observation['worker_report'].to_s.start_with?('[Subagent stopped after 4 tool rounds without a final report. Last progress shown below.]')
      exchange = @ledger.state['requests'].index(receipts.last)
      return false unless exchange
      path = File.join(@directory, format('response-%04d.json', exchange + 1))
      response = JSON.parse(JSON.parse(File.read(path)).fetch('body'))
      raise Denied, 'round-limit provider response changed' unless AgentRsiLive.digest(response) == receipts.last['response_sha256']
      final = response.fetch('choices').first
      final['finish_reason'] == 'tool_calls' && final.dig('message', 'content').to_s.empty? &&
        final.dig('message', 'tool_calls').is_a?(Array) && !final.dig('message', 'tool_calls').empty?
    end

    def native_reservation_proof(native, receipts)
      return nil unless receipts.all? { |r| %w[actual_tokens reserved_input_tokens].all? { |key| r[key].is_a?(Integer) && r[key] >= 0 } }
      scalars = {'model' => MODEL, 'stream' => false, 'temperature' => 0, 'max_tokens' => LIMITS[:output_tokens]}
      delta = FRAMING_ALLOWANCE - (JSON.generate(scalars).bytesize - 1)
      return nil unless delta.positive? && receipts.zip(native).all? do |root, observed|
        exchange = @ledger.state['requests'].index(root)
        next false unless exchange
        body = JSON.parse(File.read(File.join(@directory, format('request-%04d.json', exchange + 1))))
        next false unless body.keys.sort == %w[max_tokens messages model stream temperature tool_choice tools] &&
          body.slice('model', 'stream', 'temperature') == scalars.slice('model', 'stream', 'temperature') && body['tool_choice'] == 'auto'
        body['max_tokens'] = LIMITS[:output_tokens]
        wire = JSON.generate(body)
        observed['sequence'] == root['sequence'] && wire.bytesize == observed['request_utf8_bytes'] &&
          Digest::SHA256.hexdigest(wire) == observed['request_sha256'] &&
          AgentRsiLive.digest(body) == root['requested_body_sha256'] && root['reserved_input_tokens'] == wire.bytesize + delta
      end
      denied = native.last
      bytes = denied['request_utf8_bytes']
      return nil unless bytes.is_a?(Integer) && (1..100_000).cover?(bytes) && denied['request_sha256'].to_s.match?(/\A[0-9a-f]{64}\z/)
      used = receipts.sum { |r| r['actual_tokens'] }
      input = bytes + delta
      return nil unless used + input >= LIMITS[:trial_tokens]
      {'native_fixed_envelope_delta' => delta, 'request_utf8_bytes' => bytes, 'required_input_reservation' => input,
       'physical_tokens_already_used' => used, 'minimum_trial_total' => used + input, 'trial_token_cap' => LIMITS[:trial_tokens]}
    rescue Errno::ENOENT, JSON::ParserError, KeyError
      nil
    end

    # A local pre-admission 403 has no upstream request, token use or bill.
    # Preserve the native false/null aggregate and bind a separate host view;
    # this must never hide a billed response withheld at the billing gate.
    def budget_validation_view(trial, observation, receipts)
      native = observation['request_receipts']
      return observation unless trial['platform'] == 'rust-native-worker' &&
        observation.dig('runtime', 'adapter') == 'rust-worker-native-broker-v1' &&
        observation['worker_status'] == 'infrastructure_error' &&
        observation['usage_complete'] == false && native.is_a?(Array) && native.length == receipts.length + 1 &&
        !receipts.empty? && receipts.all? { |r| r['status'] == 'settled' && r['verified_bill_microusd'].is_a?(Integer) }
      denial = native.last
      return observation unless receipts.length <= LIMITS[:trial_requests] && denial['sequence'] == receipts.length + 1 &&
        denial['status'] == 403 && denial['transport_valid'] == false &&
        %w[receipt_id request_id response_id routed_model usage].all? { |key| denial.key?(key) && denial[key].nil? }
      physical = native[0...-1]
      return observation unless physical.all? { |r| r['status'] == 200 && r['transport_valid'] == true &&
        %w[receipt_id request_id routed_model].all? { |key| r[key].is_a?(String) && !r[key].empty? } }
      return observation unless physical.map { |r| r['receipt_id'] }.sort == receipts.map { |r| r['receipt_id'] }.sort
      last = physical.last
      truncated = observation['did_truncate'] == true && last['completion_truncated'] == true && %w[length max_tokens incomplete content_filter].include?(last['finish_reason'])
      reservation = native_reservation_proof(native, receipts) unless truncated
      return observation unless truncated || reservation
      exchange = @ledger.state['requests'].index(receipts.last)
      return observation unless exchange
      raw_response = JSON.parse(File.read(File.join(@directory, format('response-%04d.json', exchange + 1))))
      response = JSON.parse(raw_response.fetch('body'))
      raise Denied, 'local denial preceding provider evidence changed' unless raw_response['http_status'] == 200 &&
        AgentRsiLive.digest(response) == receipts.last['response_sha256'] &&
        response.dig('choices', 0, 'finish_reason') == last['finish_reason']
      if truncated
        raise Denied, 'delivered truncated content changed' unless response.dig('choices', 0, 'message', 'content') == observation['raw_model_content']
      else
        return observation unless last['finish_reason'] == 'tool_calls' && observation['raw_model_content'].nil? &&
          response.dig('choices', 0, 'message', 'tool_calls').is_a?(Array) && !response.dig('choices', 0, 'message', 'tool_calls').empty?
      end
      view = observation.merge('request_receipts' => physical, 'usage_complete' => true,
        'input_tokens' => receipts.sum { |r| r.fetch('input_tokens') },
        'output_tokens' => receipts.sum { |r| r.fetch('output_tokens') },
        'total_tokens' => receipts.sum { |r| r.fetch('actual_tokens') })
      raise Denied, 'local denial physical usage incomplete' unless complete_usage?(view, receipts)
      projection = {'schema' => 'willdeep.agent-rsi.host-budget-usage-view.v1',
        'trial_id' => trial['trial_id'], 'original_observation_sha256' => AgentRsiLive.digest(observation),
        'derived_observation_sha256' => AgentRsiLive.digest(view), 'physical_request_count' => receipts.length,
        'unadmitted_local_sequence' => denial['sequence'], 'local_denial_model_calls' => 0,
        'physical_receipt_ids' => receipts.map { |r| r['receipt_id'] },
        'input_tokens' => view['input_tokens'], 'output_tokens' => view['output_tokens'], 'total_tokens' => view['total_tokens']}
      projection['input_reservation_proof'] = reservation if reservation
      path = File.join(@directory, trial.fetch('trial_id'), 'budget-usage-projection.json')
      if File.exist?(path)
        raise Denied, 'host usage projection changed' unless JSON.parse(File.read(path)) == projection
      else
        AgentRsiLive.write(path, projection)
      end
      view
    end

    # Recovery never replays a completed or budget-limited sample. The signed
    # manifest binds this rule before further paid requests, with v1 preserved.
    def prior_terminal!(trial)
      rows = @ledger.state['attempts'].select { |row| row['trial_id'] == trial['trial_id'] }
      return false if rows.empty?
      raise Denied, 'ambiguous or unresolved planned terminal' unless rows.length == 1 && KNOWN_TERMINALS.include?(rows.first['status'])
      row = rows.first
      trial.each { |key, value| raise Denied, 'prior planned identity changed' unless row[key] == value }
      directory = File.join(@directory, trial.fetch('trial_id'))
      observation = JSON.parse(File.read(File.join(directory, 'observation.json')))
      receipts = @ledger.state['requests'].select { |record| record['trial_id'] == trial['trial_id'] }
      raise Denied, 'prior terminal contains unverified consumption' unless !receipts.empty? && receipts.all? { |record| record['status'] == 'settled' && record['verified_bill_microusd'].is_a?(Integer) }
      task = JSON.parse(File.read(@manifest.fetch('suite_path'))).fetch('tasks').find { |item| item['task_id'] == trial['task_id'] }
      view = budget_validation_view(trial, observation, receipts)
      validate_observation!(trial, view, receipts, task)
      raise Denied, 'prior terminal usage evidence incomplete' unless complete_usage?(view, receipts)
      process = JSON.parse(File.read(File.join(@directory, trial['trial_id'] + '-process.json')))
      raise Denied, 'prior native process failed' unless process['exit_status'] == 0
      if row['status'] == 'budget_exhausted'
        raise Denied, 'budget terminal evidence incomplete' unless budget_terminal?(trial, observation, receipts)
        raise Denied, 'budget terminal must not have a quality score' if File.exist?(File.join(directory, 'verifier.json'))
      else
        verification = JSON.parse(File.read(File.join(directory, 'verifier.json')))
        raise Denied, 'prior quality evidence changed' unless observation['did_truncate'] == false &&
          verification['observation_sha256'] == AgentRsiLive.digest(observation) &&
          verification['passed'] == (row['status'] == 'passed')
      end
      true
    rescue Errno::ENOENT, JSON::ParserError, KeyError
      raise Denied, 'prior terminal artifact missing or invalid'
    end

    def holdout_ready!(plan)
      prior = plan.reject { |trial| trial['split'] == 'holdout' }
      raise Denied, 'holdout before complete nonholdout terminal evidence' unless prior.length == 216 &&
        prior.map { |trial| trial['trial_id'] }.uniq.length == 216 && prior.all? { |trial| prior_terminal!(trial) }
    end

    # This preparation does not create an approval, launch a broker, read a
    # model key, or send inference. Freeze the returned manifest externally
    # only AFTER free native previews and direct account-price approval.
    def self.prepare(directory, rust_binary:, mac_root:, mac_binary:, mac_frozen:, account_snapshot:, budget_cap_microusd: 5_000_000)
      raise Denied, 'experiment directory must be new and under /private/tmp' unless directory.start_with?('/private/tmp/') && !File.exist?(directory)
      FileUtils.mkdir_p(directory, mode: 0o700)
      suite_path = File.join(ROOT, 'bench/agent-rsi/reviewer-original-24-v1/suite.json')
      suite = JSON.parse(File.read(suite_path))
      raise Denied, 'suite changed' unless AgentRsiLive.digest(suite) == SUITE
      golden = JSON.parse(File.read(File.join(ROOT, 'docs/examples/agent-rsi-golden.json'))).dig('values', 'candidate')
      packages = REVISIONS.keys.to_h do |arm|
        package = golden.fetch(arm + '_package')
        raise Denied, 'package changed' unless package['sha256'] == REVISIONS[arm]
        path = File.join(directory, arm + '.json')
        AgentRsiLive.write(path, package)
        [arm, path]
      end
      frozen_suite = File.join(directory, 'suite.json')
      AgentRsiLive.write(frozen_suite, suite)
      plan = []
      %w[validation regression holdout].each do |split|
        suite['tasks'].select { |task| task['split'] == split }.each_with_index do |task, task_index|
          1.upto(3) do |repetition|
            %w[rust-native-worker macos-native-worker].each_with_index do |platform, platform_index|
              arms = (task_index + repetition + platform_index).even? ? %w[baseline candidate] : %w[candidate baseline]
              arms.each do |arm|
                plan << {'trial_id' => SecureRandom.uuid, 'platform' => platform, 'task_id' => task['task_id'],
                         'split' => split, 'arm' => arm, 'repetition' => repetition, 'attempt' => 1,
                         'resume_of' => nil, 'agent_revision' => REVISIONS.fetch(arm)}
              end
            end
          end
        end
      end
      raise Denied, 'planned count changed' unless plan.length == 288
      AgentRsiLive.write(File.join(directory, 'plan.json'), plan)
      manifest = {'schema' => 'willdeep.agent-rsi.local-live-manifest.v1', 'experiment_id' => File.basename(directory),
                  'run_directory' => File.realpath(directory),
                  'prepared_at' => Time.now.utc.iso8601, 'proposal_sha256' => PROPOSAL, 'suite_sha256' => SUITE,
                  'suite_path' => frozen_suite, 'packages' => packages, 'plan_sha256' => AgentRsiLive.digest(plan),
                  'revisions' => REVISIONS, 'model' => MODEL, 'parameters' => {'temperature' => 0, 'stream' => false},
                  'limits' => LIMITS.transform_keys(&:to_s).merge('microdollars' => budget_cap_microusd), 'account' => account_snapshot,
                  'budget_cap_microusd' => budget_cap_microusd,
                  'maximum_bill_microusd_per_million_tokens' => account_snapshot.fetch('maximum_bill_microusd_per_million_tokens'),
                  'route_scope' => 'external gateway header; internal configured target is a separate configuration fact',
                  'configured_internal_target' => 'b7813442-14c3-49e8-957c-50b649df2dff',
                  'expected_routed_model' => MODEL, 'expected_response_model' => 'deepseek-flash',
                  'expected_gateway_version' => '2.5.0, 0.248.0-rc3',
                  'rust_binary' => File.realpath(rust_binary), 'rust_binary_sha256' => Digest::SHA256.file(rust_binary).hexdigest,
                  'mac_root' => File.realpath(mac_root), 'mac_binary' => File.realpath(mac_binary),
                  'mac_binary_sha256' => Digest::SHA256.file(mac_binary).hexdigest,
                  'mac_frozen_runner' => File.realpath(mac_frozen), 'mac_frozen_runner_sha256' => Digest::SHA256.file(mac_frozen).hexdigest,
                  'mac_scripts' => %w[agent_rsi_approved_trial.rb agent_rsi_frozen_runner.rb].to_h { |name| [name, Digest::SHA256.file(File.join(mac_root, 'scripts', name)).hexdigest] },
                  'ruby_version' => RUBY_VERSION, 'json_version' => JSON::VERSION,
                  'source_files' => SOURCE_FILES.to_h { |path| [path, Digest::SHA256.file(File.join(ROOT, path)).hexdigest] },
                  'paid_calls' => 0, 'authorization' => 'not granted by this file',
                  'claim_limit' => 'Original small-suite feasibility; no general improvement or automatic promotion.'}
      AgentRsiLive.write(File.join(directory, 'manifest.json'), manifest)
      manifest
    end

    # public_key is supplied by the trusted root operator, separately from the
    # editable run directory. The generic offline product API remains unable to
    # mint this grant or issue an inference capability.
    def initialize(directory, signed_grant:, public_key:)
      @directory = directory
      @manifest = JSON.parse(File.read(File.join(directory, 'manifest.json')))
      raise Denied, 'signed experiment cannot be replayed in another directory' unless File.realpath(directory) == @manifest['run_directory']
      grant = JSON.parse(File.read(signed_grant))
      payload = grant.fetch('payload')
      valid = OpenSSL::PKey.read(public_key).verify(OpenSSL::Digest::SHA256.new,
               grant.fetch('signature_base64').unpack1('m0'), AgentRsiLive.canonical(payload))
      raise Denied, 'missing trusted, scope-specific grant' unless valid &&
        payload['manifest_sha256'] == AgentRsiLive.digest(@manifest) && payload['proposal_sha256'] == PROPOSAL &&
        payload['model'] == MODEL && payload['budget_cap_microusd'] == @manifest.fetch('budget_cap_microusd', 5_000_000) &&
        payload['budget_cap_microusd'].is_a?(Integer) && (1..20_000_000).cover?(payload['budget_cap_microusd']) &&
        payload['maximum_bill_microusd_per_million_tokens'] == @manifest['maximum_bill_microusd_per_million_tokens'] &&
        payload['direct_human_confirmation_source'].is_a?(String) && !payload['direct_human_confirmation_source'].empty? &&
        Time.iso8601(payload.fetch('expires_at')) > Time.now
      raise Denied, 'resume grant does not bind exact existing ledger' unless payload['resume_ledger_sha256'] == @manifest['resume_ledger_sha256']
      verify_frozen_files
      @account = Account.new
      check_price!
      @ledger = Ledger.new(directory, @manifest, expires_at: Time.iso8601(payload.fetch('expires_at')), resume_from_sha256: payload['resume_ledger_sha256'])
      @broker = Broker.new(@ledger, upstream: @account.base.sub(%r{/+$}, '') + '/chat/completions',
                           api_key: @account.broker_key, directory: directory,
                           expected_response_model: @manifest['expected_response_model'], expected_gateway_version: @manifest['expected_gateway_version'],
                           billing_validator: ->(record) { reconcile_bills({'trial_id' => record.fetch('trial_id')}, [record]) })
    end

    def verify_frozen_files
      %w[rust mac].each do |platform|
        raise Denied, 'native build changed after freezing' unless Digest::SHA256.file(@manifest.fetch(platform + '_binary')).hexdigest == @manifest.fetch(platform + '_binary_sha256')
      end
      @manifest.fetch('source_files').each do |path, hash|
        raise Denied, 'host or scoring rules changed after freezing' unless Digest::SHA256.file(File.join(ROOT, path)).hexdigest == hash
      end
      raise Denied, 'frozen Mac execution manifest changed' unless Digest::SHA256.file(@manifest.fetch('mac_frozen_runner')).hexdigest == @manifest.fetch('mac_frozen_runner_sha256')
      @manifest.fetch('mac_scripts').each do |name, hash|
        raise Denied, 'Mac native launcher changed' unless Digest::SHA256.file(File.join(@manifest.fetch('mac_root'), 'scripts', name)).hexdigest == hash
      end
      raise Denied, 'host interpreter changed' unless @manifest['ruby_version'] == RUBY_VERSION && @manifest['json_version'] == JSON::VERSION
      raise Denied, 'frozen suite changed' unless AgentRsiLive.digest(JSON.parse(File.read(@manifest.fetch('suite_path')))) == SUITE
      plan = JSON.parse(File.read(File.join(@directory, 'plan.json')))
      raise Denied, 'frozen plan changed' unless AgentRsiLive.digest(plan) == @manifest.fetch('plan_sha256')
    end

    def check_price!
      reference = @manifest.dig('account', 'pricing_reference_request_id')
      raise Denied, 'static model catalog does not verify time-policy billing; historical evidence required' unless reference
      current = @account.snapshot(pricing_reference_request_id: reference)
      %w[scope_sha256 bill_input_per_1m bill_output_per_1m billing_multiplier cost_margin maximum_bill_microusd_per_million_tokens].each do |key|
        raise Denied, 'account or pricing changed; no more inference permitted' unless current[key] == @manifest.dig('account', key)
      end
    end

    def run
      plan = JSON.parse(File.read(File.join(@directory, 'plan.json')))
      holdout_checked = false
      plan.each do |trial|
        next if recovery_v2? && prior_terminal!(trial)
        if trial['split'] == 'holdout'
          if recovery_v2?
            holdout_ready!(plan) unless holdout_checked
            holdout_checked = true
          else
            prior = @ledger.state['attempts'].select { |row| row['split'] != 'holdout' }
            raise Denied, 'holdout before complete nonholdout evidence' unless prior.count { |row| QUALITY_TERMINALS.include?(row['status']) } == 216
          end
        end
        verify_frozen_files
        check_price!
        run_trial(trial)
        break if @ledger.state['paused']
      end
      report
    rescue StandardError => error
      @ledger.finish_trial('incomplete') if @ledger.state['attempts'].last&.fetch('status') == 'running'
      @ledger.pause(error.is_a?(Denied) ? error.message : 'host failure; manual reconciliation required')
      report
    ensure
      @broker.close
      @ledger.close
    end

    def run_trial(trial)
      @ledger.begin_trial(trial)
      if recovery_v2?
        @ledger.state['attempts'].last['scoring_protocol'] = 'v2-prospective'
        @ledger.save
      end
      directory = File.join(@directory, trial.fetch('trial_id'))
      control = {'schema' => 'willdeep.agent-rsi.native-trial-control.v1', 'proposal_sha256' => PROPOSAL,
                 'experiment_id' => @manifest.fetch('experiment_id'), 'suite' => @manifest.fetch('suite_path'),
                 'suite_sha256' => SUITE, 'agent_package' => @manifest.fetch('packages').fetch(trial['arm']),
                 'broker_origin' => @broker.origin, 'capability' => @broker.activate(trial.fetch('trial_id')),
                 'output_directory' => directory}.merge(trial.reject { |key, _| %w[platform split].include?(key) })
      control_path = File.join(@directory, 'active-control.json')
      AgentRsiLive.write(control_path, control)
      if trial['platform'] == 'rust-native-worker'
        command = [@manifest.fetch('rust_binary'), 'agents', 'rsi', 'native-trial', '--control', control_path]
        working_directory = ROOT
      else
        command = ['ruby', File.join(@manifest.fetch('mac_root'), 'scripts/agent_rsi_approved_trial.rb'), '--control', control_path, '--execute', '--frozen-runner', @manifest.fetch('mac_frozen_runner')]
        working_directory = @manifest.fetch('mac_root')
      end
      output, error, status = Open3.capture3(*command, chdir: working_directory)
      AgentRsiLive.write(File.join(@directory, trial['trial_id'] + '-process.json'),
                         {'exit_status' => status.exitstatus, 'stdout_sha256' => Digest::SHA256.hexdigest(output), 'stderr_sha256' => Digest::SHA256.hexdigest(error)})
      raise Denied, 'native process failed; no automatic paid replay' unless status.success? && File.exist?(File.join(directory, 'observation.json'))
      observation = JSON.parse(File.read(File.join(directory, 'observation.json')))
      receipts = @ledger.state['requests'].select { |row| row['trial_id'] == trial['trial_id'] }
      suite = JSON.parse(File.read(@manifest.fetch('suite_path')))
      task = suite.fetch('tasks').find { |item| item['task_id'] == trial['task_id'] }
      view = recovery_v2? ? budget_validation_view(trial, observation, receipts) : observation
      validate_observation!(trial, view, receipts, task)
      raise Denied, 'native usage evidence incomplete' if recovery_v2? && !complete_usage?(view, receipts)
      if recovery_v2? && budget_terminal?(trial, observation, receipts)
        @ledger.finish_trial('budget_exhausted')
        reconcile_bills(trial, receipts)
        prior_terminal!(trial)
        puts JSON.generate(terminal: 'budget_exhausted', platform: trial['platform'], arm: trial['arm'], quality_score: nil)
        $stdout.flush
        return
      end
      unless observation['did_truncate'] == false && observation['status'] == 'unverified' &&
             (!observation.key?('worker_status') || observation['worker_status'] == 'completed')
        @ledger.finish_trial(observation['did_truncate'] ? 'budget_exhausted' : 'incomplete')
        raise Denied, 'native terminal is truncated or incomplete; no ordinary quality score'
      end
      raw = observation['raw_model_content']
      raise Denied, 'missing final native output; no quality score' unless raw.is_a?(String)
      exchange = @ledger.state['requests'].index(receipts.last) + 1
      upstream = JSON.parse(File.read(File.join(@directory, format('response-%04d.json', exchange))))
      final = JSON.parse(upstream.fetch('body')).fetch('choices').first.fetch('message')
      raise Denied, 'native answer not bound to last provider response' unless final['content'] == raw && (final['tool_calls'].nil? || final['tool_calls'].empty?)
      expected = JSON.parse(task.fetch('expected_output'))
      verification = Grader.grade(raw, expected, read_observed: observation['read_observed'], fixture_unchanged: observation['fixture_unchanged'])
      verification['observation_sha256'] = AgentRsiLive.digest(observation)
      verification['trial_id'] = trial['trial_id']
      verification['scoring_protocol'] = 'v2-prospective' if recovery_v2?
      AgentRsiLive.write(File.join(directory, 'verifier.json'), verification)
      reconcile_bills(trial, receipts)
      @ledger.finish_trial(verification['passed'] ? 'passed' : 'quality_failure')
      puts JSON.generate(completed: @ledger.state['attempts'].count { |row| %w[passed quality_failure].include?(row['status']) }, planned: 288, platform: trial['platform'],
                         arm: trial['arm'], passed: verification['passed'],
                         accounted_microusd: @ledger.state['requests'].sum { |row| Ledger.exposure(row) })
      $stdout.flush
    ensure
      @broker.deactivate
      File.delete(control_path) if control_path && File.exist?(control_path)
    end

    def validate_observation!(trial, observation, receipts, task)
      identity = trial.merge('proposal_sha256' => PROPOSAL, 'suite_sha256' => SUITE,
                             'experiment_id' => @manifest.fetch('experiment_id'),
                             'definition_revision' => trial['agent_revision'], 'loaded_revision' => trial['agent_revision'])
      identity.reject { |key, _| %w[split agent_revision].include?(key) }.each do |key, expected|
        raise Denied, "native observation identity missing or changed: #{key}" unless observation[key] == expected
      end
      raise Denied, 'unexpected native observation schema' unless %w[willdeep.agent-rsi.native-observation.v1 willdeep.agent-rsi.native-trial-observation.v1].include?(observation['schema'])
      raise Denied, 'preview cannot be scored as live execution' unless observation['preview'].nil? || observation['preview'] == false
      raise Denied, 'task evidence missing or changed' unless observation['task_sha256'] == AgentRsiLive.digest(task)
      raise Denied, 'fixture evidence missing or changed' unless observation['fixture_sha256'] == Digest::SHA256.hexdigest(task.fetch('fixture'))
      raise Denied, 'read/integrity evidence missing' unless [true, false].include?(observation['read_observed']) && [true, false].include?(observation['fixture_unchanged'])
      context = observation['context'] || observation
      root, worker = context['root_run_id'], context['worker_run_id']
      uuid = /\A[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\z/i
      raise Denied, 'root/worker lineage missing or aliased' unless root.is_a?(String) && worker.is_a?(String) && root.match?(uuid) && worker.match?(uuid) && root.downcase != worker.downcase
      events = observation['events']
      raise Denied, 'execution events missing or altered' unless events.is_a?(Array) && observation['events_sha256'] == AgentRsiLive.digest(events)
      raise Denied, 'effective prompt evidence missing' unless observation['effective_prompt_sha256'].is_a?(String) && observation['effective_prompt_sha256'].match?(/\A[0-9a-f]{64}\z/)
      platform = trial['platform'] == 'rust-native-worker' ? 'rust' : 'mac'
      raise Denied, 'executed native binary differs from frozen artifact' unless observation.dig('runtime', 'binary_sha256') == @manifest.fetch(platform + '_binary_sha256')
      native = observation['request_receipts']
      raise Denied, 'native receipts missing' unless native.is_a?(Array)
      raise Denied, 'no complete provider receipts for native trial' if receipts.empty? || receipts.any? { |row| row['status'] != 'settled' }
      raise Denied, 'native receipt identity missing' unless native.all? { |row| %w[receipt_id request_id routed_model].all? { |key| row[key].is_a?(String) && !row[key].empty? } }
      expected = receipts.map { |row| [row['receipt_id'], row['request_id'], row['routed_model']] }.sort
      actual = native.map { |row| [row['receipt_id'], row['request_id'], row['routed_model']] }.sort
      raise Denied, 'native receipts do not match root broker' unless actual == expected
    end

    def reconcile_bills(trial, receipts)
      return if receipts.all? { |receipt| receipt.key?('verified_bill_microusd') }
      bills = @account.billing_receipts(trial['trial_id'])
      receipts.each do |receipt|
        bill = bills.find { |item| item['request_id'] == receipt['request_id'] }
        raise Denied, 'billing receipt pending or missing; pause with conservative exposure' unless bill && bill['cost']
        receipt['billing_receipt'] = bill
        @ledger.save
        amount = BigDecimal(bill.fetch('cost'))
        raise Denied, 'invalid billing amount; manual reconciliation required' unless amount.finite? && amount >= 0
        actual = (amount * 1_000_000).ceil
        receipt['observed_bill_microusd'] = actual
        @ledger.save
        raise Denied, 'unexpected server model fallback' unless bill['model'] == MODEL && bill['failover_from'].to_s.empty?
        if @manifest&.dig('account', 'pricing_reference_request_id')
          diagnostic = @account.pricing_diagnostic(receipt.fetch('request_id'))
          receipt['billing_diagnostic_sha256'] = Digest::SHA256.hexdigest(JSON.generate(diagnostic))
          pricing = diagnostic.fetch('pricing_snapshot')
          receipt['actual_pricing_snapshot'] = pricing
          @ledger.save
          factor = BigDecimal(pricing.fetch('policy_adjustment').fetch('billing_multiplier').to_s) * BigDecimal(bill.fetch('billing_multiplier'))
          margin = BigDecimal(pricing.fetch('cost_margin').to_s)
          input_rate = BigDecimal(pricing.fetch('input_price_per_1m').to_s) * margin * factor
          output_rate = BigDecimal(pricing.fetch('output_price_per_1m').to_s) * margin * factor
          raise Denied, 'effective billed price exceeds approved rate ceiling' unless input_rate <= BigDecimal(@manifest.dig('account', 'bill_input_per_1m')) && output_rate <= BigDecimal(@manifest.dig('account', 'bill_output_per_1m'))
        end
        raise Denied, 'actual bill exceeds approved account-rate bound' if actual > receipt.fetch('accounted_microusd')
        receipt['verified_bill_microusd'] = actual
      end
      @ledger.save
    end

    def report
      state = @ledger.state
      rows = state['attempts'].map do |trial|
        path = File.join(@directory, trial['trial_id'], 'verifier.json')
        trial.merge('verification' => File.exist?(path) ? JSON.parse(File.read(path)) : nil,
                    'scoring_epoch' => trial.fetch('scoring_protocol', 'v1-historical'))
      end
      plan = JSON.parse(File.read(File.join(@directory, 'plan.json')))
      historical_ids = @manifest.fetch('historical_trial_ids', [])
      groups = rows.group_by { |trial| [trial['platform'], trial['arm'], trial['scoring_epoch']] }.map do |(platform, arm, epoch), trials|
        complete = trials.select { |trial| %w[passed quality_failure].include?(trial['status']) }
        metrics = %w[schema_valid classification_correct localization_correct read_observed fixture_unchanged passed]
        planned = plan.count { |trial| trial['platform'] == platform && trial['arm'] == arm &&
          (!recovery_v2? || historical_ids.include?(trial['trial_id']) == (epoch == 'v1-historical')) }
        {'platform' => platform, 'arm' => arm, 'scoring_epoch' => epoch, 'completed' => complete.length,
         'quality_inference_eligible' => !recovery_v2? || epoch == 'v2-prospective',
         'planned_samples' => planned,
         'observed_attempts' => trials.length, 'budget_exhausted' => trials.count { |trial| trial['status'] == 'budget_exhausted' },
         'metrics' => metrics.to_h { |key| [key, complete.count { |trial| trial.dig('verification', key) == true }] }}
      end
      plan_ids = plan.map { |trial| trial['trial_id'] }
      report = {'schema' => 'willdeep.agent-rsi.local-live-report.v1', 'planned_trials' => 288,
                'protocol_revision' => @manifest.fetch('protocol_revision', 'v1'),
                'terminal_planned_trials' => rows.count { |trial| plan_ids.include?(trial['trial_id']) && KNOWN_TERMINALS.include?(trial['status']) },
                'budget_exhausted_trials' => rows.count { |trial| trial['status'] == 'budget_exhausted' },
                'completed_trials' => rows.count { |trial| %w[passed quality_failure].include?(trial['status']) },
                'paused' => state['paused'], 'requests' => state['requests'].length, 'groups' => groups, 'trials' => rows,
                'paired_classification' => paired_comparison(recovery_v2? ? rows.select { |row| row['scoring_epoch'] == 'v2-prospective' } : rows),
                'accounted_microusd' => state['requests'].sum { |row| Ledger.exposure(row) },
                'verified_bill_microusd' => state['requests'].all? { |row| row.key?('verified_bill_microusd') } ? state['requests'].sum { |row| row['verified_bill_microusd'] } : nil,
                'unknown_requests' => state['requests'].count { |row| row['status'] != 'settled' }, 'automatic_promotion' => false,
                'budget_cap_microusd' => @manifest.fetch('budget_cap_microusd', 5_000_000),
                'unreconciled_bills' => state['requests'].count { |row| !row.key?('verified_bill_microusd') },
                'claim_limit' => @manifest['claim_limit']}
      AgentRsiLive.write(File.join(@directory, 'report.json'), report)
      AgentRsiLive.write(File.join(@directory, 'report.md'), "# 首轮原生审查实验\n\n计划终态 #{report['terminal_planned_trials']}/288；可评分观察 #{report['completed_trials']}；预算耗尽 #{report['budget_exhausted_trials']}；模型请求 #{report['requests']}。\n\n暂停原因：#{report['paused'] || '无'}。\n\n按冻结最高账单价保守核算 $#{format('%.6f', report['accounted_microusd'] / 1_000_000.0)}；已核实实扣：#{report['verified_bill_microusd'].nil? ? '未全部核实' : format('$%.6f', report['verified_bill_microusd'] / 1_000_000.0)}。\n\nv1 历史与 v2 后续评分分组保留，不混入事后免费回放。预算终态不重试、不计普通质量分；配对要求同评分版本两臂三重复完整。分类、精确行、格式及实际读取分别保留于 JSON；不自动晋升。\n")
      report
    end

    # Repetitions are nested within a task, not 72 independent problems. Match
    # all three repetitions of BOTH arms before a task enters the comparison.
    # A fixed-seed cluster bootstrap shows small-suite uncertainty; it is not a
    # promotion gate or evidence for general model improvement.
    def paired_comparison(rows)
      rows.group_by { |row| [row['platform'], row['scoring_epoch']] }.map do |(platform, epoch), platform_rows|
        differences = platform_rows.group_by { |row| row['task_id'] }.filter_map do |_task, trials|
          arms = %w[baseline candidate].map do |arm|
            entries = trials.select { |row| row['arm'] == arm && %w[passed quality_failure].include?(row['status']) }
            next unless entries.map { |row| row['repetition'] }.sort == [1, 2, 3]
            entries.count { |row| row.dig('verification', 'classification_correct') == true } / 3.0
          end
          arms[1] - arms[0] if arms.none?(&:nil?)
        end
        random = Random.new(15_102_026)
        bootstrap = if differences.empty?
          []
        else
          Array.new(5000) { Array.new(differences.length) { differences[random.rand(differences.length)] }.sum / differences.length }.sort
        end
        {'platform' => platform, 'scoring_epoch' => epoch, 'independent_task_pairs' => differences.length,
         'candidate_minus_baseline_accuracy' => differences.empty? ? nil : differences.sum / differences.length,
         'bootstrap_task_cluster_95_percent_interval' => bootstrap.empty? ? nil : [bootstrap[124], bootstrap[4874]],
         'task_wins' => differences.count(&:positive?), 'task_ties' => differences.count(&:zero?),
         'task_losses' => differences.count(&:negative?), 'repetitions_are_not_independent_tasks' => true}
      end
    end
  end
end
