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
  VERSION_LINE = 'willdeep-multica-adapter 1.0.1'
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
    when 'usage', 'subagent_usage'
      usage = usage_block(event)
      usage_model = event['model']
      usage && usage_model.is_a?(String) && !usage_model.empty? ?
        [assistant_message([], model: usage_model, usage:)] : []
    else
      []
    end
  rescue JSON::ParserError
    []
  end

  # willdeep 的收尾行加上退出码，翻成对方要的唯一终态行。没有收尾行时按退出码说明原因。
  def self.result_event(completion, exit_code:, session_id:)
    completion = nil unless completion.is_a?(Hash) && %w[completed partial].include?(completion['type'])
    text = completion&.fetch('text', nil)
    text = '' unless text.is_a?(String)
    stop = completion&.fetch('stop_reason', nil)
    failed = completion.nil? || exit_code != 0 || INCOMPLETE_STOPS.include?(stop)
    error = failure_text(exit_code, stop, text)
    error += '; missing completed/partial terminal event' if completion.nil?

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
    { 'type' => 'system', 'subtype' => 'init', 'session_id' => session_id }.compact
  end

  # 只交回 willdeep 本次确实报告的会话 id；失效的 resume 不能在失败
  # result 里被原样回传，否则 Multica 不会重试新会话。
  def self.session_id_for(reported)
    reported if reported.is_a?(String) && reported.match?(UUID_PATTERN)
  end

  # 先 fork 监护进程，再由它启动 willdeep：Multica 的 CommandContext
  # 即便 SIGKILL 适配器，管道 EOF 也会让监护进程清理整个工具进程组。
  def self.run_child(command, arguments, prompt)
    prompt_read, prompt_write = IO.pipe
    output_read, output_write = IO.pipe
    status_read, status_write = IO.pipe
    watch_read, watch_write = IO.pipe
    watcher = fork do
      prompt_write.close
      output_read.close
      status_read.close
      watch_write.close
      STDOUT.reopen(File::NULL, 'w')
      STDERR.reopen(File::NULL, 'w')
      begin
        # 父进程已死就不要再启动一个新任务。
        exit! 0 if IO.select([watch_read], nil, nil, 0) && watch_read.eof?
        # stderr 可能含工具输出，不能进入 Multica 的 daemon 日志。
        child_pid = Process.spawn(command, *arguments, '--input', '-', in: prompt_read,
                                  out: output_write, err: File::NULL, pgroup: true)
        prompt_read.close
        output_write.close
        waiter = Thread.new do
          status = Process.waitpid2(child_pid).last
          status_write.write((status.exitstatus || 1).to_s)
          status_write.close
        end
        terminate_group(child_pid) unless watch_read.read == 'done'
        waiter.join
      rescue SystemCallError
        status_write.write('5') unless status_write.closed?
      ensure
        status_write.close unless status_write.closed?
        output_write.close unless output_write.closed?
        exit! 0
      end
    end
    prompt_read.close
    output_write.close
    status_write.close
    watch_read.close
    handlers = %w[TERM INT HUP].to_h do |signal|
      [signal, Signal.trap(signal) { raise Interrupt, signal }]
    end
    finished = false
    begin
      prompt_write.write(prompt)
      prompt_write.close
      output_read.each_line { |line| yield line }
      status = Integer(status_read.read)
      finished = true
      status
    ensure
      handlers.each { |signal, previous| Signal.trap(signal, previous) }
      prompt_write.close unless prompt_write.closed?
      output_read.close unless output_read.closed?
      status_read.close unless status_read.closed?
      watch_write.write('done') if finished
      watch_write.close
      Process.waitpid(watcher)
    end
  end

  def self.terminate_group(pid)
    Process.kill('TERM', -pid)
    sleep 0.3
    Process.kill('KILL', -pid)
  rescue Errno::ESRCH
    nil
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
      { 'type' => 'thinking', 'text' => text }
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
      # 成功事件没有输出字段；detail 是调用参数摘要，不是执行结果。失败事件
      # 可能含命令和输出，仅给消费端一个固定状态，绝不转发可能含密钥的原文。
      result = {
        'type' => 'tool_result',
        'tool_use_id' => event['id'].is_a?(String) ? event['id'] : '',
        'is_error' => errored
      }
      result['content'] = { 'error' => 'Tool failed' } if errored
      result
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
    completion = nil
    tool_started = false
    # 先报无 ID 的运行状态，终态确定会话后再补一条 system；事件逐行
    # 交付，不为长任务积压整份输出，也不把旧 resume id 冒充新会话。
    $stdout.sync = true
    puts JSON.generate(MulticaClaudeAdapter.system_event(nil))
    status = MulticaClaudeAdapter.run_child(willdeep, arguments, prompt) do |line|
      parsed = begin
        JSON.parse(line)
      rescue JSON::ParserError
        nil
      end
      next unless parsed.is_a?(Hash)

      if %w[completed partial].include?(parsed['type'])
        completion = parsed
        next
      end
      tool_started ||= %w[tool_requested tool_completed].include?(parsed['type'])
      MulticaClaudeAdapter.translate_event(line, session_id: nil).each do |event|
        puts JSON.generate(event)
      end
    end
    session_id = MulticaClaudeAdapter.session_id_for(completion&.fetch('session_id', nil))
    exit_code = completion.nil? && status.zero? ? 5 : status
    result = MulticaClaudeAdapter.result_event(completion, exit_code: exit_code, session_id: session_id)
    # 已开始工具的失败可能留下写操作；重放提示词会执行第二遍。
    # 只在确认没有工具调用时允许 Multica 对失效 resume 重试。
    if requested && tool_started && result['is_error'] && session_id != requested
      session_id = requested
      result['session_id'] = session_id
    end
    puts JSON.generate(MulticaClaudeAdapter.system_event(session_id)) if session_id
    puts JSON.generate(result)
    warn("multica adapter ignored: #{dropped.join(', ')}") unless dropped.empty?
    exit(exit_code)
  rescue MulticaClaudeAdapter::UsageError => error
    warn "multica adapter: #{error.message}"
    exit 2
  rescue Interrupt => error
    warn "multica adapter: interrupted by #{error.message}"
    exit 130
  end
end
