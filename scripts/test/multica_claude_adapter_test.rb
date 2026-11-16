#!/usr/bin/env ruby
# frozen_string_literal: true

# 适配器的协议翻译测试。不跑 willdeep，也不联网：
#
#   ruby scripts/test/multica_claude_adapter_test.rb
#
# 钉住的是 daemon 会读的那几个形状：stream-json 的 argv、stdin 上的 user 消息、
# assistant 的 text 与 tool_use 块、user 的 tool_result 块、唯一的 result 终态行，
# 以及 usage 不能把缓存命中计两次。

require 'json'
require 'minitest/autorun'
require 'open3'
require 'stringio'
require 'tmpdir'
require 'timeout'

require_relative '../multica_claude_adapter'

ADAPTER = File.expand_path('../multica_claude_adapter.rb', __dir__)
MULTICA_ROOT = ENV.fetch('MULTICA_ROOT', File.expand_path('~/Sites/multica'))
CONTRACT_PROBE = File.expand_path('multica_claude_contract.go', __dir__)
SESSION = '11111111-1111-1111-1111-111111111111'

DAEMON_ARGV = [
  '-p', '--output-format', 'stream-json', '--input-format', 'stream-json', '--verbose',
  '--strict-mcp-config', '--permission-mode', 'bypassPermissions',
  '--disallowedTools', 'AskUserQuestion'
].freeze

def daemon_argv(*extra)
  DAEMON_ARGV + extra
end

class TranslateArgvTest < Minitest::Test
  def test_daemon_argv_becomes_a_headless_run
    action, arguments, dropped = MulticaClaudeAdapter.translate(daemon_argv)

    assert_equal :run, action
    assert_equal %w[run --output ndjson], arguments
    assert_includes dropped, '--verbose'
    assert_includes dropped, '--strict-mcp-config'
    assert_includes dropped, '--disallowedTools <value>'
  end

  def test_resume_and_model_are_forwarded
    _action, arguments, = MulticaClaudeAdapter.translate(
      daemon_argv('--resume', SESSION, '--model', 'claude-opus-4-7')
    )

    assert_equal ['run', '--output', 'ndjson', '--session', SESSION, '--model', 'claude-opus-4-7'],
                 arguments
  end

  def test_resume_accepts_equals_form_and_rejects_non_uuids
    _action, arguments, = MulticaClaudeAdapter.translate(daemon_argv("--resume=#{SESSION}"))
    assert_includes arguments, SESSION

    _action, arguments, = MulticaClaudeAdapter.translate(daemon_argv('--resume', 'latest'))
    refute_includes arguments, '--session'
  end

  def test_unknown_flags_pass_through_with_their_values
    _action, arguments, = MulticaClaudeAdapter.translate(
      daemon_argv('--workspace', '/srv/repo', '--max-turns', '12')
    )

    assert_equal ['run', '--output', 'ndjson', '--workspace', '/srv/repo', '--max-turns', '12'],
                 arguments
  end

  def test_version_probe_is_reported_separately
    assert_equal [:version], MulticaClaudeAdapter.translate(['--version'])
    assert_equal [:version], MulticaClaudeAdapter.translate(['-v'])
  end

  def test_wrong_permission_mode_is_rejected
    argv = daemon_argv.map { |token| token == 'bypassPermissions' ? 'default' : token }
    error = assert_raises(MulticaClaudeAdapter::UsageError) { MulticaClaudeAdapter.translate(argv) }
    assert_match(/permission-mode/, error.message)
  end

  def test_missing_stream_format_is_rejected
    assert_raises(MulticaClaudeAdapter::UsageError) { MulticaClaudeAdapter.translate(['-p']) }
  end
end

class PromptTest < Minitest::Test
  def test_text_blocks_join_into_the_prompt
    raw = JSON.generate(
      'type' => 'user',
      'message' => {
        'role' => 'user',
        'content' => [
          { 'type' => 'text', 'text' => 'fix the build' },
          { 'type' => 'image', 'source' => { 'type' => 'base64' } },
          { 'type' => 'text', 'text' => ' and run tests' }
        ]
      }
    )

    assert_equal 'fix the build and run tests', MulticaClaudeAdapter.prompt_from_stdin(StringIO.new(raw))
  end

  def test_empty_stdin_and_non_user_messages_are_rejected
    assert_raises(MulticaClaudeAdapter::UsageError) { MulticaClaudeAdapter.prompt_from_stdin(StringIO.new('')) }
    assert_raises(MulticaClaudeAdapter::UsageError) do
      MulticaClaudeAdapter.prompt_from_stdin(StringIO.new('{"type":"assistant"}'))
    end
    assert_raises(MulticaClaudeAdapter::UsageError) do
      MulticaClaudeAdapter.prompt_from_stdin(StringIO.new('not json'))
    end
  end
end

class TranslateEventTest < Minitest::Test
  def translate(event)
    MulticaClaudeAdapter.translate_event(JSON.generate(event), session_id: SESSION)
  end

  def test_text_delta_becomes_an_assistant_text_block
    events = translate('type' => 'assistant_text_delta', 'text' => 'looking', 'model' => 'glm-5')

    assert_equal 1, events.length
    message = events.first.fetch('message')
    assert_equal 'assistant', events.first.fetch('type')
    assert_equal 'glm-5', message.fetch('model')
    assert_equal [{ 'type' => 'text', 'text' => 'looking' }], message.fetch('content')
  end

  def test_full_assistant_text_is_not_repeated
    assert_empty translate('type' => 'assistant_text', 'text' => 'looking')
  end

  def test_reasoning_becomes_a_thinking_block
    events = translate('type' => 'reasoning_delta', 'text' => 'hmm')

    assert_equal [{ 'type' => 'thinking', 'text' => 'hmm' }], events.first.dig('message', 'content')
  end

  def test_tool_request_and_completion_keep_the_call_id
    requested = translate(
      'type' => 'tool_requested', 'id' => 'call-1', 'name' => 'read_file',
      'expansion' => '{"path":"README.md"}', 'detail' => 'read README.md'
    )
    block = requested.first.dig('message', 'content', 0)
    assert_equal 'tool_use', block.fetch('type')
    assert_equal 'call-1', block.fetch('id')
    assert_equal 'read_file', block.fetch('name')
    assert_equal({ 'path' => 'README.md' }, block.fetch('input'))

    completed = translate('type' => 'tool_completed', 'id' => 'call-1', 'name' => 'read_file',
                          'is_error' => false, 'detail' => 'read README.md')
    result = completed.first.dig('message', 'content', 0)
    assert_equal 'user', completed.first.fetch('type')
    assert_equal 'tool_result', result.fetch('type')
    assert_equal 'call-1', result.fetch('tool_use_id')
    assert_equal false, result.fetch('is_error')
    refute result.key?('content'), 'success has no tool output; detail is only the input summary'
  end

  def test_failed_tool_carries_the_reason
    events = translate(
      'type' => 'tool_completed', 'id' => 'call-2', 'name' => 'run_command', 'is_error' => true,
      'arguments' => '{"command":"rm -rf /"}', 'output' => 'approval denied'
    )
    result = events.first.dig('message', 'content', 0)

    assert_equal true, result.fetch('is_error')
    assert_equal({ 'error' => 'Tool failed' }, result.fetch('content'))
    refute_includes JSON.generate(result), 'rm -rf'
    refute_includes JSON.generate(result), 'approval denied'
  end

  def test_unparseable_expansion_stays_an_object
    events = translate('type' => 'tool_requested', 'name' => 'run_command', 'expansion' => 'cargo test')

    assert_equal({ 'expansion' => 'cargo test' }, events.first.dig('message', 'content', 0, 'input'))
  end

  def test_usage_subtracts_cache_reads_and_drops_contradictions
    events = translate('type' => 'usage', 'model' => 'actual-model', 'input_tokens' => 100,
                       'output_tokens' => 5, 'cache_read_tokens' => 80)
    assert_equal 'actual-model', events.first.dig('message', 'model')
    usage = events.first.dig('message', 'usage')
    assert_equal 20, usage.fetch('input_tokens')
    assert_equal 5, usage.fetch('output_tokens')
    assert_equal 80, usage.fetch('cache_read_input_tokens')
    assert_equal 0, usage.fetch('cache_creation_input_tokens')

    assert_empty translate('type' => 'usage', 'model' => 'actual-model', 'input_tokens' => 10,
                           'cache_read_tokens' => 50)
    assert_empty translate('type' => 'usage', 'input_tokens' => 10)
    child = translate('type' => 'subagent_usage', 'model' => 'worker-model', 'input_tokens' => 8,
                      'output_tokens' => 2, 'cache_read_tokens' => 3)
    assert_equal 'worker-model', child.first.dig('message', 'model')
    assert_equal 5, child.first.dig('message', 'usage', 'input_tokens')
  end

  def test_unknown_and_broken_lines_are_ignored
    assert_empty translate('type' => 'compression_started', 'estimated_tokens' => 10)
    assert_empty MulticaClaudeAdapter.translate_event('not json', session_id: SESSION)
  end
end

class ResultEventTest < Minitest::Test
  def result(completion, exit_code)
    MulticaClaudeAdapter.result_event(completion, exit_code: exit_code, session_id: SESSION)
  end

  def test_finished_run_is_a_success
    event = result({ 'type' => 'completed', 'stop_reason' => 'finished', 'text' => 'done', 'turns' => 3 }, 0)

    assert_equal 'result', event.fetch('type')
    assert_equal false, event.fetch('is_error')
    assert_equal 'done', event.fetch('result')
    assert_equal SESSION, event.fetch('session_id')
    assert_equal 3, event.fetch('num_turns')
  end

  def test_unconverged_run_is_an_error_even_with_exit_zero
    event = result({ 'type' => 'partial', 'stop_reason' => 'max_turns', 'text' => 'halfway' }, 0)

    assert_equal true, event.fetch('is_error')
    assert_match(/max_turns/, event.fetch('result'))
    assert_match(/halfway/, event.fetch('result'))
  end

  def test_exit_codes_name_the_cause
    assert_match(/provider/, result(nil, 3).fetch('result'))
    assert_match(/approval or policy/, result(nil, 4).fetch('result'))
    assert_match(/rejected the input/, result(nil, 2).fetch('result'))
  end

  def test_session_id_requires_a_valid_report_from_willdeep
    reported = '22222222-2222-2222-2222-222222222222'
    assert_equal reported, MulticaClaudeAdapter.session_id_for(reported)
    assert_nil MulticaClaudeAdapter.session_id_for('not-a-uuid')
    assert_nil MulticaClaudeAdapter.session_id_for(nil)
  end
end

class EndToEndTest < Minitest::Test
  PROMPT = { 'type' => 'user', 'message' => { 'role' => 'user',
                                              'content' => [{ 'type' => 'text', 'text' => 'fix it' }] } }.freeze

  def run_adapter(willdeep_lines, exit_code:, argv: daemon_argv)
    Dir.mktmpdir do |dir|
      stub = File.join(dir, 'willdeep')
      File.write(stub, stub_source(willdeep_lines, exit_code))
      File.chmod(0o755, stub)

      Open3.capture3({ 'WILLDEEP_BIN' => stub }, 'ruby', ADAPTER, *argv, stdin_data: JSON.generate(PROMPT))
    end
  end

  def events_from(stdout)
    stdout.lines.map { |line| JSON.parse(line) }
  end

  # 桩的源码用 %q 包住每一行，避免把 JSON 里的引号交回去再转义一次。
  def stub_source(lines, exit_code, stderr: nil)
    body = lines.map { |line| "    STDOUT.puts(%q{#{line}})" }.join("\n")
    body += "\n    warn(#{stderr.inspect})" if stderr
    <<~RUBY
      #!/usr/bin/env ruby
      if ARGV == ['--version']
        puts 'willdeep 0.148.0-rc1'
        exit 0
      end
      abort("unexpected argv: \#{ARGV.inspect}") unless ARGV.include?('--output') && ARGV.include?('ndjson')
      abort('prompt missing') unless \$stdin.read == 'fix it'
      #{body}
      exit #{exit_code}
    RUBY
  end

  def test_a_run_streams_then_ends_with_one_result
    lines = [
      JSON.generate('type' => 'turn_started', 'turn' => 1),
      JSON.generate('type' => 'assistant_text_delta', 'text' => 'reading', 'model' => 'glm-5'),
      JSON.generate('type' => 'tool_requested', 'id' => 'call-1', 'name' => 'read_file',
                    'expansion' => '{"path":"a.rs"}'),
      JSON.generate('type' => 'tool_completed', 'id' => 'call-1', 'name' => 'read_file', 'is_error' => false),
      JSON.generate('type' => 'usage', 'model' => 'actual-model', 'input_tokens' => 30,
                    'output_tokens' => 4, 'cache_read_tokens' => 10),
      JSON.generate('type' => 'completed', 'stop_reason' => 'finished', 'text' => 'fixed', 'turns' => 1,
                    'session_id' => SESSION)
    ]
    stdout, stderr, status = run_adapter(lines, exit_code: 0)

    assert_equal 0, status.exitstatus, stderr
    events = events_from(stdout)
    assert_equal 'system', events.first.fetch('type')
    refute events.first.key?('session_id')
    assert_equal 'system', events[-2].fetch('type')
    assert_equal SESSION, events[-2].fetch('session_id')
    assert_equal 'result', events.last.fetch('type')
    assert_equal false, events.last.fetch('is_error')
    assert_equal 'fixed', events.last.fetch('result')
    assert_equal SESSION, events.last.fetch('session_id')
    assert_equal %w[assistant assistant user assistant], events[1..-3].map { |event| event.fetch('type') }
    assert_match(/disallowedTools/, stderr)
  end

  def test_version_probe_prints_one_line_and_never_runs_a_turn
    stdout, stderr, status = run_adapter([], exit_code: 9, argv: ['--version'])

    assert_equal 0, status.exitstatus, stderr
    assert_match(/\d+\.\d+\.\d+/, stdout)
    assert_equal '', stderr
  end

  def test_a_refused_run_is_reported_as_an_error_result
    lines = [JSON.generate('type' => 'completed', 'stop_reason' => 'incomplete', 'text' => '', 'turns' => 0,
                           'session_id' => SESSION)]
    stdout, _stderr, status = run_adapter(lines, exit_code: 4)

    assert_equal 4, status.exitstatus
    result = events_from(stdout).last
    assert_equal true, result.fetch('is_error')
    assert_match(/approval or policy/, result.fetch('result'))
  end

  def test_bad_input_exits_before_starting_willdeep
    _stdout, stderr, status = Open3.capture3('ruby', ADAPTER, '-p', stdin_data: '{}')

    assert_equal 2, status.exitstatus
    assert_match(/output-format/, stderr)
  end

  def test_exit_zero_without_terminal_line_is_a_failure
    stdout, _stderr, status = run_adapter([JSON.generate('type' => 'assistant_text_delta', 'text' => 'half')],
                                          exit_code: 0)
    assert_equal 5, status.exitstatus
    assert_equal true, events_from(stdout).last.fetch('is_error')
    assert_match(/missing completed\/partial terminal event/, events_from(stdout).last.fetch('result'))
  end

  def test_failed_resume_does_not_echo_a_session_id_that_never_started
    stdout, _stderr, status = run_adapter([], exit_code: 2, argv: daemon_argv('--resume', SESSION))
    assert_equal 2, status.exitstatus
    events = events_from(stdout)
    refute events.first.key?('session_id')
    refute events.last.key?('session_id')
    assert_equal true, events.last.fetch('is_error')
  end

  def test_text_is_visible_before_willdeep_finishes
    Dir.mktmpdir do |dir|
      stub = File.join(dir, 'willdeep')
      live_line = JSON.generate('type' => 'assistant_text_delta', 'text' => 'live')
      File.write(stub, <<~RUBY)
        #!/usr/bin/env ruby
        STDIN.read
        puts #{live_line.inspect}
        STDOUT.flush
        sleep 30
      RUBY
      File.chmod(0o755, stub)
      Open3.popen3({ 'WILLDEEP_BIN' => stub }, 'ruby', ADAPTER, *daemon_argv) do |input, output, _err, waiter|
        input.write(JSON.generate(PROMPT))
        input.close
        first, text = Timeout.timeout(5) { [JSON.parse(output.gets), JSON.parse(output.gets)] }
        assert_equal 'system', first.fetch('type')
        assert_equal 'live', text.dig('message', 'content', 0, 'text')
        refute waiter.join(0), 'adapter must still be running when text arrives'
        Process.kill('TERM', waiter.pid)
        Timeout.timeout(3) { waiter.value }
      ensure
        Process.kill('KILL', waiter.pid) unless waiter.join(0)
      end
    end
  end

  def test_failed_resume_with_a_started_tool_cannot_replay_side_effects
    lines = [JSON.generate('type' => 'tool_requested', 'id' => 'write-1', 'name' => 'edit_file')]
    stdout, _stderr, status = run_adapter(lines, exit_code: 0, argv: daemon_argv('--resume', SESSION))
    assert_equal 5, status.exitstatus
    events = events_from(stdout)
    assert_equal SESSION, events[-2].fetch('session_id')
    assert_equal SESSION, events.last.fetch('session_id')
    assert_equal true, events.last.fetch('is_error')
  end

  def test_multica_real_go_consumer_parses_events_and_usage
    skip 'local Multica server checkout unavailable' unless File.file?(File.join(MULTICA_ROOT, 'server/go.mod'))
    skip 'Go compiler unavailable' unless system('go version', out: File::NULL, err: File::NULL)

    lines = [
      JSON.generate('type' => 'assistant_text_delta', 'text' => 'working'),
      JSON.generate('type' => 'reasoning_delta', 'text' => 'checking'),
      JSON.generate('type' => 'tool_requested', 'id' => 'call-1', 'name' => 'read_file',
                    'expansion' => '{"path":"a.rs"}'),
      JSON.generate('type' => 'tool_completed', 'id' => 'call-1', 'name' => 'read_file',
                    'detail' => 'read a.rs', 'is_error' => false),
      JSON.generate('type' => 'tool_completed', 'id' => 'call-2', 'name' => 'run_command',
                    'arguments' => 'sensitive arguments', 'output' => 'sensitive output', 'is_error' => true),
      JSON.generate('type' => 'usage', 'model' => 'actual-model', 'input_tokens' => 30,
                    'output_tokens' => 4, 'cache_read_tokens' => 10),
      JSON.generate('type' => 'subagent_usage', 'model' => 'worker-model', 'input_tokens' => 8,
                    'output_tokens' => 2, 'cache_read_tokens' => 3),
      JSON.generate('type' => 'completed', 'stop_reason' => 'finished', 'text' => 'fixed',
                    'session_id' => SESSION, 'turns' => 1)
    ]
    Dir.mktmpdir do |dir|
      stub = File.join(dir, 'willdeep')
      File.write(stub, stub_source(lines, 0, stderr: 'sensitive stderr'))
      File.chmod(0o755, stub)
      stdout, stderr, status = Open3.capture3('go', 'run', CONTRACT_PROBE, ADAPTER, stub,
                                              chdir: File.join(MULTICA_ROOT, 'server'))
      assert status.success?, stderr
      observed = JSON.parse(stdout)
      result = observed.fetch('result')
      assert_equal 'completed', result.fetch('Status')
      assert_equal SESSION, result.fetch('SessionID')
      assert_equal 'fixed', result.fetch('Output')
      usage = result.fetch('Usage').fetch('actual-model')
      assert_equal 20, usage.fetch('InputTokens')
      assert_equal 10, usage.fetch('CacheReadTokens')
      assert_equal 4, usage.fetch('OutputTokens')
      worker = result.fetch('Usage').fetch('worker-model')
      assert_equal 5, worker.fetch('InputTokens')
      assert_equal 3, worker.fetch('CacheReadTokens')
      messages = observed.fetch('messages')
      assert messages.any? { |msg| msg['Type'] == 'thinking' && msg['Content'] == 'checking' }
      assert messages.any? { |msg| msg['Type'] == 'tool-use' && msg['CallID'] == 'call-1' }
      tool_result = messages.find { |msg| msg['Type'] == 'tool-result' }
      assert_equal 'call-1', tool_result.fetch('CallID')
      assert_equal '', tool_result.fetch('Output'), 'a success without output must not show its input summary'
      failed_result = messages.find { |msg| msg['Type'] == 'tool-result' && msg['CallID'] == 'call-2' }
      assert_equal '{"error":"Tool failed"}', failed_result.fetch('Output')
      refute_includes JSON.generate(observed), 'sensitive output'
      refute_includes JSON.generate(observed), 'sensitive arguments'
    end
  end

  def test_multica_real_go_consumer_falls_back_after_invalid_resume
    skip 'local Multica server checkout unavailable' unless File.file?(File.join(MULTICA_ROOT, 'server/go.mod'))
    Dir.mktmpdir do |dir|
      stub = File.join(dir, 'willdeep')
      File.write(stub, stub_source([], 2))
      File.chmod(0o755, stub)
      stdout, stderr, status = Open3.capture3('go', 'run', CONTRACT_PROBE, ADAPTER, stub, SESSION,
                                              chdir: File.join(MULTICA_ROOT, 'server'))
      assert status.success?, stderr
      result = JSON.parse(stdout).fetch('result')
      assert_equal 'failed', result.fetch('Status')
      assert_equal '', result.fetch('SessionID')
    end
  end

  def test_sigkill_and_sigterm_reap_willdeep_and_its_tool_child
    %w[KILL TERM].each do |signal|
      Dir.mktmpdir do |dir|
        ready = File.join(dir, 'ready')
        escaped = File.join(dir, 'escaped')
        stub = File.join(dir, 'willdeep')
        File.write(stub, <<~RUBY)
          #!/usr/bin/env ruby
          child = fork { sleep 1.2; File.write(#{escaped.inspect}, 'still running') }
          File.write(#{ready.inspect}, "\#{Process.pid}:\#{child}")
          sleep 30
        RUBY
        File.chmod(0o755, stub)
        reader, writer = IO.pipe
        pid = Process.spawn({ 'WILLDEEP_BIN' => stub }, 'ruby', ADAPTER, *daemon_argv,
                            in: reader, out: File::NULL, err: File::NULL)
        reader.close
        writer.write(JSON.generate(PROMPT))
        writer.close
        Timeout.timeout(5) { sleep 0.02 until File.exist?(ready) }
        Process.kill(signal, pid)
        Timeout.timeout(5) { Process.waitpid(pid) }
        sleep 1.4
        refute File.exist?(escaped), "#{signal} left a willdeep tool running"
      ensure
        begin
          Process.kill('KILL', pid) if pid && Process.waitpid(pid, Process::WNOHANG).nil?
        rescue Errno::ESRCH, Errno::ECHILD
          nil
        end
        if File.exist?(ready)
          group = File.read(ready).split(':').first.to_i
          begin
            Process.kill('KILL', -group) if group.positive?
          rescue Errno::ESRCH
            nil
          end
        end
      end
    end
  end
end
