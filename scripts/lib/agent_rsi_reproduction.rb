# frozen_string_literal: true
require 'json'
require 'digest'
require 'open3'
require 'timeout'

# Standalone, free-only host verification. Never loads live/Account/provider code.
module AgentRsiReproduction
  ROOT = File.expand_path('../..', __dir__)
  FIXTURE_ROOT = File.join(ROOT, 'bench/agent-rsi/reproduction-free-v1')
  MANIFEST_SHA256 = '4cb96af334f1d8a7113240e8cbe7c4e275009d68233ce6cab52b67926ce05af3'
  HARNESS = File.join(__dir__, 'agent_rsi_reproduction_harness.js')
  HARNESS_SHA256 = 'a6a45b706044c2daaf28c8759a7699a3c18e7d2d28c0f08afda69073a93eb59b'
  MAX_BYTES = 16_384
  MAX_DEPTH = 8
  MAX_ITEMS = 128
  MAX_INTEGER = 9_007_199_254_740_991
  RESERVED_KEYS = %w[__proto__ prototype constructor].freeze
  NODE_PATHS = %w[/opt/homebrew/bin/node /usr/local/bin/node /usr/bin/node].freeze
  REPORT_KEYS = %w[has_defect line reason reproduction].freeze
  EVIDENCE_KEYS = %w[delivered_to_model fixture_sha256 fixture_unchanged read_observed].freeze
  class Denied < StandardError; end
  class UniqueObject < Hash
    def []=(key, value)
      raise JSON::ParserError, 'duplicate_key' if key?(key)
      super
    end
  end
  module_function

  def hash(value)
    normalized = case value
                 when Hash then value.keys.sort.to_h { |key| [key, normalize(value[key])] }
                 else normalize(value)
                 end
    Digest::SHA256.hexdigest(JSON.generate(normalized))
  end

  def normalize(value)
    case value
    when Hash then value.keys.sort.to_h { |key| [key, normalize(value[key])] }
    when Array then value.map { |item| normalize(item) }
    else value
    end
  end

  def lexical_integers!(raw)
    quoted = escaped = false
    index = 0
    while index < raw.length
      character = raw[index]
      if quoted
        if escaped then escaped = false
        elsif character == '\\' then escaped = true
        elsif character == '"' then quoted = false
        end
      elsif character == '"' then quoted = true
      elsif character.match?(/[-0-9]/)
        token = raw[index..].match(/\A[-+0-9.eE]+/)[0]
        raise Denied, 'noncanonical_or_unsafe_integer' unless token.match?(/\A(?:0|[1-9][0-9]*)\z/) && token.to_i <= MAX_INTEGER
        index += token.length
        next
      end
      index += 1
    end
  end

  def bounded_data!(value, depth = 1, counter = [0])
    raise Denied, 'json_depth_limit' if depth > MAX_DEPTH
    case value
    when Hash
      raise Denied, 'reserved_object_key' if value.keys.any? { |key| RESERVED_KEYS.include?(key) }
      raise Denied, 'string_size_limit' if value.keys.any? { |key| key.bytesize > 4096 }
      counter[0] += value.length
      value.each_value { |item| bounded_data!(item, depth + 1, counter) }
    when Array
      counter[0] += value.length
      value.each { |item| bounded_data!(item, depth + 1, counter) }
    when Integer then raise Denied, 'unsafe_integer' unless value.between?(0, MAX_INTEGER)
    when String then raise Denied, 'string_size_limit' if value.bytesize > 4096
    when NilClass, TrueClass, FalseClass
    else raise Denied, 'invalid_json_data_type'
    end
    raise Denied, 'collection_item_limit' if counter[0] > MAX_ITEMS
    value
  end

  def parse_report(raw)
    raise Denied, 'report_size_or_encoding' unless raw.is_a?(String) && raw.encoding != Encoding::ASCII_8BIT && raw.valid_encoding? && raw.bytesize <= MAX_BYTES
    lexical_integers!(raw)
    report = JSON.parse(raw, object_class: UniqueObject, allow_duplicate_key: false, max_nesting: MAX_DEPTH)
    bounded_data!(report)
    raise Denied, 'report_exact_fields' unless report.is_a?(Hash) && report.keys.sort == REPORT_KEYS
    raise Denied, 'report_field_types' unless [true, false].include?(report['has_defect']) && report['reason'].is_a?(String) && !report['reason'].strip.empty?
    if report['has_defect']
      raise Denied, 'defect_requires_line_and_reproduction' unless report['line'].is_a?(Integer) && report['line'].positive? && report['reproduction'].is_a?(Hash) && report['reproduction'].keys.sort == %w[actual expected input]
    else
      raise Denied, 'clean_requires_null_line_and_reproduction' unless report['line'].nil? && report['reproduction'].nil?
    end
    report
  rescue JSON::ParserError, JSON::NestingError
    raise Denied, 'invalid_or_duplicate_json'
  end

  def trusted_file(root, relative, digest)
    path = File.realpath(File.join(root, relative))
    raise Denied, 'untrusted_source_path' unless path.start_with?(File.realpath(root) + '/')
    bytes = File.binread(path).force_encoding(Encoding::UTF_8)
    raise Denied, 'trusted_source_digest_mismatch' unless bytes.valid_encoding? && Digest::SHA256.hexdigest(bytes) == digest
    bytes
  rescue Errno::ENOENT
    raise Denied, 'trusted_source_missing'
  end

  def load_task(case_id, fixture_root = FIXTURE_ROOT)
    bytes = File.binread(File.join(fixture_root, 'manifest.json'))
    raise Denied, 'untrusted_manifest' unless Digest::SHA256.hexdigest(bytes) == MANIFEST_SHA256
    task = JSON.parse(bytes).fetch('tasks').find { |item| item['case_id'] == case_id }
    raise Denied, 'unknown_public_case' unless task
    [task, trusted_file(fixture_root, task['fixture_file'], task['fixture_sha256']),
     trusted_file(fixture_root, task['reference_file'], task['reference_sha256'])]
  end

  def validate_input!(input, shape)
    bounded_data!(input)
    raise Denied, 'input_requires_object' unless input.is_a?(Hash)
    valid = case shape
            when 'count_fallback'
              input.keys.sort == %w[count fallback] && (input['count'].nil? || input['count'].is_a?(Integer)) && input['fallback'].is_a?(Integer)
            when 'numeric_values'
              input.keys == ['values'] && input['values'].is_a?(Array) && input['values'].length <= 32 && input['values'].all? { |item| item.is_a?(Integer) }
            when 'key_entries'
              input.keys == ['entries'] && input['entries'].is_a?(Array) && input['entries'].length <= 24 && input['entries'].all? do |entry|
                entry.is_a?(Hash) && entry.keys.sort == %w[key value] && entry['key'].is_a?(String) && entry['key'].match?(/\A[a-z][a-z0-9_]{0,15}\z/) &&
                  !RESERVED_KEYS.include?(entry['key']) && entry['value'].is_a?(Integer)
              end
            else false
            end
    raise Denied, 'input_shape_or_size' unless valid
    input
  end

  def execute(task, fixture_source, reference_source, input)
    raise Denied, 'untrusted_harness' unless Digest::SHA256.file(HARNESS).hexdigest == HARNESS_SHA256
    node = NODE_PATHS.find { |path| File.executable?(path) }
    raise Denied, 'node_unavailable' unless node
    payload = JSON.generate('fixture_source' => fixture_source, 'fixture_sha256' => task['fixture_sha256'],
                            'reference_source' => reference_source, 'reference_sha256' => task['reference_sha256'], 'input' => input)
    output = status = nil
    Open3.popen3(node, '--max-old-space-size=32', HARNESS, pgroup: true) do |stdin, stdout, stderr, process|
      begin
        Timeout.timeout(2) do
          stdin.write(payload)
          stdin.close
          output = stdout.read(MAX_BYTES + 1)
          stderr.read(1025)
          status = process.value
        end
      rescue Timeout::Error
        begin
          Process.kill('KILL', -process.pid)
        rescue Errno::ESRCH
          # Child already exited while the deadline was being enforced.
        end
        raise Denied, 'trusted_execution_timeout'
      ensure
        [stdin, stdout, stderr].each { |stream| stream.close unless stream.closed? }
      end
    end
    raise Denied, 'execution_output_size' if output.bytesize > MAX_BYTES
    result = JSON.parse(output)
    raise Denied, result.fetch('execution_error', 'trusted_execution_failed') unless status.success?
    bounded_data!(result)
    result
  rescue JSON::ParserError
    raise Denied, 'invalid_execution_result'
  end
  private_class_method :execute

  def base_result(case_id, raw)
    {'schema' => 'willdeep.agent-rsi.reproduction-verifier.v1', 'execution_mode' => 'offline_mock_only',
     'case_id' => case_id, 'format_valid' => false, 'classification_correct' => nil, 'line_correct' => nil,
     'read_observed' => false, 'delivered_to_model' => false, 'fixture_unchanged' => false, 'evidence_verified' => false,
     'reproduction_verified' => false, 'reproduction_status' => 'unverified', 'objective_execution_count' => 0,
     'passed' => false, 'model_calls' => 0, 'cost_microusd' => 0,
     'report_sha256' => raw.is_a?(String) ? Digest::SHA256.hexdigest(raw) : nil,
     'claim_limit' => 'Public engineering fixtures only; no semantic reason judging, paid inference, general quality improvement or promotion claim.'}
  end

  def grade(raw, task_id:, evidence:, fixture_root: FIXTURE_ROOT)
    result = base_result(task_id, raw)
    task, fixture, reference = load_task(task_id, fixture_root)
    result.merge!('fixture_sha256' => task['fixture_sha256'], 'reference_sha256' => task['reference_sha256'])
    report = parse_report(raw)
    result.merge!('format_valid' => true, 'classification_correct' => report['has_defect'] == task['has_defect'],
                  'line_correct' => report['line'] == task['defect_line'])
    evidence_ok = evidence.is_a?(Hash) && evidence.keys.sort == EVIDENCE_KEYS && evidence['fixture_sha256'] == task['fixture_sha256']
    result['read_observed'] = evidence_ok && evidence['read_observed'] == true
    result['delivered_to_model'] = evidence_ok && evidence['delivered_to_model'] == true
    result['fixture_unchanged'] = evidence_ok && evidence['fixture_unchanged'] == true
    result['evidence_verified'] = result['read_observed'] && result['delivered_to_model'] && result['fixture_unchanged']
    inputs = report['has_defect'] ? [report['reproduction']['input']] : task['public_probes']
    runs = inputs.map do |input|
      validate_input!(input, task['input_shape'])
      executed = execute(task, fixture, reference, input)
      result['objective_execution_count'] += 2
      {'input_sha256' => hash(input), 'fixture_actual' => executed['fixture_actual'], 'reference_expected' => executed['reference_expected']}
    end
    result['host_executions'] = runs
    if report['has_defect']
      reproduction = report['reproduction']
      actual, expected = runs.first.values_at('fixture_actual', 'reference_expected')
      verified = reproduction['actual'] == actual && reproduction['expected'] == expected && actual != expected
      result.merge!('reproduction_verified' => verified, 'reproduction_status' => verified ? 'verified' : 'failed')
    else
      result.merge!('reproduction_verified' => nil, 'reproduction_status' => 'not_applicable_clean')
      result['clean_public_probes_match'] = runs.all? { |run| run['fixture_actual'] == run['reference_expected'] }
    end
    objective = report['has_defect'] ? result['reproduction_verified'] : result['clean_public_probes_match']
    result['passed'] = result['classification_correct'] && result['line_correct'] && result['evidence_verified'] && objective
    result['verifier_input_sha256'] = hash('report_sha256' => result['report_sha256'], 'evidence' => evidence,
                                          'fixture_sha256' => task['fixture_sha256'], 'reference_sha256' => task['reference_sha256'], 'runs' => runs)
    result
  rescue Denied => error
    result.merge('error_code' => error.message)
  end
end
