#!/usr/bin/env ruby
# frozen_string_literal: true

# Multica 把 willdeep 当成 Claude Code 来调。
#
# Multica 不接新协议，自定义 runtime 只能套一个它已经会说的协议族。这个脚本冒充 `claude` 二进制，
# 讲 daemon 期待的 stream-json（见 multica-ai/multica 的 server/pkg/agent/claude.go），
# 内里调 `willdeep run`。注册方式见 docs/MULTICA_ADAPTER.md。
#
#   ruby scripts/multica_claude_adapter.rb [daemon 传来的 claude 参数...]
#
# 退出码沿用 willdeep 的：0 完成，2 输入错，3 provider，4 审批或策略拒绝，5 其余。
# 协议本身读不懂（缺提示词、权限模式不是 bypassPermissions）也是 2。

require 'json'

module MulticaClaudeAdapter
  VERSION_LINE = 'willdeep-multica-adapter 1.0.0'
  PERMISSION_MODES = %w[bypassPermissions acceptEdits dontAsk].freeze
  DROPPED_FLAGS = %w[
    --verbose --strict-mcp-config --include-partial-messages --allowedTools
    --disallowedTools --mcp-config --effort --append-system-prompt
    --append-system-prompt-file --settings --fallback-model --betas
    --replay-user-messages
  ].freeze
  DROPPED_WITH_VALUE = %w[--mcp-config --effort --allowedTools --disallowedTools
                          --settings --fallback-model --betas].freeze
  # 对方认的终态只有一种：退出码 0 加一行 type=result。这些 stop_reason 说明
  # 模型没有收敛，交出去会被看板当成做完了。
  INCOMPLETE_STOPS = %w[partial budget_limited max_turns incomplete unverified].freeze
  UUID_PATTERN = /\A[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\z/i

  # daemon 拼出的 argv 翻成 willdeep 的。返回
  # [:version] 或 [:run, willdeep 参数数组, 丢弃项数组]。
  def self.translate(argv)
    flags = parse_flags(argv)
    return [:version] if flags[:version] || flags[:help]

    raise UsageError, 'missing --output-format stream-json' unless flags[:output_format] == 'stream-json'
    raise UsageError, 'missing --input-format stream-json' unless flags[:input_format] == 'stream-json'
    unless PERMISSION_MODES.include?(flags[:permission_mode])
      raise UsageError, "--permission-mode must be one of #{PERMISSION_MODES.join(', ')}"
    end

    run = ['run', '--output', 'ndjson']
    resume = flags[:resume]
    # 续跑 id 必须是我们上回交回去的会话 UUID。别的值会让 willdeep 去找一个
    # 不存在的会话，所以这里丢掉，当作新会话。
    run += ['--session', resume] if resume&.match?(UUID_PATTERN)
    model = flags[:model]
    run += ['--model', model] if model
    run += flags[:passthrough]
    [:run, run, flags[:dropped]]
  end

  # stdin 上的一行 Claude 协议提示词，抽出文本。空、多行、不是 user 消息都拒绝。
  def self.prompt_from_stdin(io)
    line = io.gets
    raise UsageError, 'no prompt on stdin' if line.nil?

    message = JSON.parse(line)
    raise UsageError, 'prompt must be one JSON object' unless message.is_a?(Hash)
    unless message['type'] == 'user'
      raise UsageError, "prompt type must be \"user\", got #{message['type'].inspect}"
    end

    content = message.dig('message', 'content')
    texts = Array(content).filter_map do |block|
      block['text'] if block.is_a?(Hash) && block['type'] == 'text' && block['text'].is_a?(String)
    end
    prompt = texts.join
    raise UsageError, 'prompt has no text content' if prompt.empty?

    prompt
  rescue JSON::ParserError => error
    raise UsageError, "prompt is not JSON: #{error.message}"
  end

  # 一行 willdeep 事件翻成零行或多行 Claude 事件。session_id 由调用方持有，
  # 第一行事件之前要先发一行 system。
  def self.translate_event(line, session_id:)
    event = JSON.parse(line)
    return [] unless event.is_a?(Hash)

    case event['type']
    when 'assistant_text_delta'
      text = event['text']
      return [] unless text.is_a?(String) && !text.empty?

      [assistant_message([text_block(text)], model: event['model'])]
    when 'reasoning_delta'
      text = event['text']
      return [] unless text.is_a?(String) && !text.empty?

      [assistant_message([thinking_block(text)])]
    when 'assistant_text'
      # 流式增量已经交付过正文；整段再交一次会让看板把回复打两遍。
      []
    when 'tool_requested'
      name = event['name']
      return [] unless name.is_a?(String) && !name.empty?

      [assistant_message([tool_use_block(event)])]
    when 'tool_completed'
      [user_message([tool_result_block(event)])]
    when 'usage'
      usage = usage_block(event)
      usage ? [assistant_message([], usage:)] : []
    else
      []
    end
  rescue JSON::ParserError
    []
  end

  # willdeep 的收尾行加上退出码，翻成对方要的唯一终态行。没有收尾行时按退出码说明原因。
  def self.result_event(completion, exit_code:, session_id:)
    completion = nil unless completion.is_a?(Hash) && completion['type']
    text = completion&.fetch('text', nil)
    text = '' unless text.is_a?(String)
    stop = completion&.fetch('stop_reason', nil)
    failed = exit_code != 0 || INCOMPLETE_STOPS.include?(stop)
    error = failure_text(exit_code, stop, text)

    {
      'type' => 'result',
      'subtype' => failed ? 'error' : 'success',
      'is_error' => failed,
      'result' => failed ? error : text,
      'session_id' => session_id,
      'num_turns' => completion&.fetch('turns', nil),
      'stop_reason' => stop
    }.compact
  end

  def self.system_event(session_id)
    { 'type' => 'system', 'subtype' => 'init', 'session_id' => session_id }
  end

  # 续跑只能用我们自己上次回传的会话 id；对方传来别的东西就当新会话，
  # 不能让 willdeep 拿着它去找一个必然不存在的会话。
  def self.session_id_for(resume, reported)
    return reported if reported.is_a?(String) && reported.match?(UUID_PATTERN)
    return resume if resume.is_a?(String) && resume.match?(UUID_PATTERN)

    'pending'
  end

  class UsageError < StandardError; end

  class << self
    private

    def parse_flags(argv)
      flags = { passthrough: [], dropped: [] }
      tokens = argv.dup
      until tokens.empty?
        token = tokens.shift
        case token
        when '--version', '-v', '-V', '--help', '-h'
          flags[:version] = true
        when '-p', '--print'
          flags[:print] = true
        when '--verbose', '--strict-mcp-config', '--include-partial-messages',
             '--replay-user-messages'
          flags[:dropped] << token
        when '--output-format', '--input-format', '--permission-mode', '--model',
             '--resume', '--mcp-config', '--effort', '--allowedTools', '--disallowedTools',
             '--append-system-prompt', '--append-system-prompt-file', '--settings',
             '--fallback-model', '--betas'
          value = tokens.shift
          flags[:dropped] << "#{token} <value>" if DROPPED_FLAGS.include?(token)
          assign_flag(flags, token, value)
        else
          if token.start_with?('--') && token.include?('=')
            name, value = token.split('=', 2)
            if known_flag?(name)
              flags[:dropped] << "#{name}=<value>" if DROPPED_FLAGS.include?(name)
              assign_flag(flags, name, value)
            else
              flags[:passthrough] << token
            end
          elsif token.start_with?('-')
            flags[:passthrough] << token
            flags[:passthrough] << tokens.shift if tokens.first && !tokens.first.start_with?('-')
          else
            flags[:passthrough] << token
          end
        end
      end
      flags
    end

    def known_flag?(name)
      %w[--output-format --input-format --permission-mode --model --resume --mcp-config
         --effort --allowedTools --disallowedTools --append-system-prompt
         --append-system-prompt-file --settings --fallback-model --betas].include?(name)
    end

    def assign_flag(flags, name, value)
      case name
      when '--output-format' then flags[:output_format] = value
      when '--input-format' then flags[:input_format] = value
      when '--permission-mode' then flags[:permission_mode] = value
      when '--model' then flags[:model] = value unless value.nil? || value.empty?
      when '--resume' then flags[:resume] = value unless value.nil? || value.empty?
      end
    end

    def assistant_message(content, model: nil, usage: nil)
      message = { 'role' => 'assistant', 'content' => content }
      message['model'] = model if model.is_a?(String) && !model.empty?
      message['usage'] = usage if usage
      { 'type' => 'assistant', 'message' => message }
    end

    def user_message(content)
      { 'type' => 'user', 'message' => { 'role' => 'user', 'content' => content } }
    end

    def text_block(text)
      { 'type' => 'text', 'text' => text }
    end

    def thinking_block(text)
      { 'type' => 'thinking', 'thinking' => text }
    end

    def tool_use_block(event)
      {
        'type' => 'tool_use',
        'id' => event['id'].is_a?(String) && !event['id'].empty? ? event['id'] : "tool-#{event['name']}",
        'name' => event['name'],
        'input' => tool_input(event)
      }
    end

    # willdeep 的工具事件不带原始参数，只有摘要和展开。展开能解析成 JSON 就用，
    # 否则把两者放进一个对象，保证 input 永远是对象。
    def tool_input(event)
      expansion = event['expansion']
      if expansion.is_a?(String) && !expansion.empty?
        parsed = JSON.parse(expansion)
        return parsed if parsed.is_a?(Hash)
      end
      input = {}
      input['detail'] = event['detail'] if event['detail'].is_a?(String)
      input['expansion'] = expansion if expansion.is_a?(String)
      input
    rescue JSON::ParserError
      { 'expansion' => event['expansion'] }
    end

    def tool_result_block(event)
      errored = event['is_error'] == true
      body = if errored
               [event['arguments'], event['output']].grep(String).reject(&:empty?).join("\n")
             else
               event['detail'].is_a?(String) ? event['detail'] : ''
             end
      {
        'type' => 'tool_result',
        'tool_use_id' => event['id'].is_a?(String) ? event['id'] : '',
        'is_error' => errored,
        'content' => body
      }
    end

    # 对方按 input 加上 output 累加，而 willdeep 的 input 已经含缓存命中。
    # 直接照搬会把缓存部分计两次，所以先扣掉。扣出负数说明数字对不上，整条丢掉。
    def usage_block(event)
      input = event['input_tokens']
      output = event['output_tokens']
      cache_read = event['cache_read_tokens']
      return nil unless [input, output, cache_read].all? { |value| value.nil? || value.is_a?(Integer) }

      input -= cache_read if input && cache_read
      return nil if [input, output, cache_read].any? { |value| value.is_a?(Integer) && value.negative? }

      {
        'input_tokens' => input || 0,
        'output_tokens' => output || 0,
        'cache_read_input_tokens' => cache_read || 0,
        'cache_creation_input_tokens' => 0
      }
    end

    def failure_text(exit_code, stop, text)
      reason = case exit_code
               when 0 then "willdeep stopped without converging (#{stop})"
               when 2 then 'willdeep rejected the input'
               when 3 then 'the model provider failed'
               when 4 then 'willdeep refused: approval or policy'
               else 'willdeep failed'
               end
      text.empty? ? "#{reason} (exit #{exit_code})" : "#{reason} (exit #{exit_code}): #{text}"
    end
  end
end

if $PROGRAM_NAME == __FILE__
  begin
    action, arguments, dropped = MulticaClaudeAdapter.translate(ARGV)
    if action == :version
      puts MulticaClaudeAdapter::VERSION_LINE
      exit 0
    end

    prompt = MulticaClaudeAdapter.prompt_from_stdin($stdin)
    willdeep = ENV.fetch('WILLDEEP_BIN', 'willdeep')
    requested = arguments.include?('--session') ? arguments[arguments.index('--session') + 1] : nil
    session_id = MulticaClaudeAdapter.session_id_for(requested, nil)
    completion = nil
    # 会话 id 只在收尾行里。先缓冲事件，等它到了再一次性写出去，
    # 这样 system 行和后面的事件用同一个 id。
    buffered = []

    IO.popen([willdeep, *arguments, '--input', '-'], 'r+') do |child|
      child.write(prompt)
      child.close_write
      child.each_line do |line|
        parsed = begin
          JSON.parse(line)
        rescue JSON::ParserError
          nil
        end
        if parsed.is_a?(Hash) && %w[completed partial].include?(parsed['type'])
          completion = parsed
          session_id = MulticaClaudeAdapter.session_id_for(requested, parsed['session_id'])
          next
        end
        buffered.concat(MulticaClaudeAdapter.translate_event(line, session_id: session_id))
      end
    end
    status = $?
    session_id = MulticaClaudeAdapter.session_id_for(requested, completion&.fetch('session_id', nil))
    puts JSON.generate(MulticaClaudeAdapter.system_event(session_id))
    buffered.each { |event| puts JSON.generate(event) }
    puts JSON.generate(MulticaClaudeAdapter.result_event(completion, exit_code: status.exitstatus || 1,
                                                                     session_id: session_id))
    warn("multica adapter ignored: #{dropped.join(', ')}") unless dropped.empty?
    exit(status.exitstatus || 1)
  rescue MulticaClaudeAdapter::UsageError => error
    warn "multica adapter: #{error.message}"
    exit 2
  end
end
