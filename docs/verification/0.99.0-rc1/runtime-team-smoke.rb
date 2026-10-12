require "json"
require "net/http"
require "open3"
require "securerandom"
require "tmpdir"
require "fileutils"
require "digest"
require "socket"

binary = File.expand_path(ARGV.fetch(0))
home = Dir.mktmpdir("willdeep-team-runtime-", "/private/tmp")
workspace = File.join(home, "workspace")
FileUtils.mkdir_p(workspace)
provider = TCPServer.new("127.0.0.1", 0)
calls = []
mutex = Mutex.new
provider_thread = Thread.new do
  loop do
    socket = provider.accept
    headers = {}
    request_line = socket.gets
    while (line = socket.gets) && line != "\r\n"
      key, value = line.split(":", 2)
      headers[key.downcase] = value.strip
    end
    if request_line.start_with?("GET ")
      body = JSON.generate(data: [{ id: "fixture", object: "model" }])
      socket.write("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: #{body.bytesize}\r\nConnection: close\r\n\r\n#{body}")
      socket.close
      next
    end
    request = JSON.parse(socket.read(Integer(headers.fetch("content-length"))))
    mutex.synchronize { calls << request }
    usage = { prompt_tokens: 10, completion_tokens: 5, total_tokens: 15 }
    content = "CONCLUSION\nFixture inspection complete.\nEVIDENCE\nNo tools were requested.\nOPEN QUESTIONS\nnone"
    if request["stream"]
      chunks = [ { choices: [{ index: 0, delta: { content: content }, finish_reason: nil }] },
        { choices: [{ index: 0, delta: {}, finish_reason: "stop" }], usage: usage } ]
      body = chunks.map { |chunk| "data: #{JSON.generate(chunk)}\n\n" }.join + "data: [DONE]\n\n"
      type = "text/event-stream"
    else
      body = JSON.generate(choices: [{ index: 0, message: { role: "assistant", content: content }, finish_reason: "stop" }], usage: usage)
      type = "application/json"
    end
    socket.write("HTTP/1.1 200 OK\r\nContent-Type: #{type}\r\nContent-Length: #{body.bytesize}\r\nConnection: close\r\n\r\n#{body}")
    socket.close
  end
rescue IOError, Errno::EBADF
end
config = File.join(home, "config.toml")
File.write(config, "version = 1\ndefault_provider = \"fixture\"\n[providers.fixture]\nprovider = \"openai-compatible\"\napi_base = \"http://127.0.0.1:#{provider.addr[1]}/v1\"\napi_key = \"fixture-not-a-real-key\"\nmodel = \"fixture\"\n[agent]\nauto_title = false\ninput_suggestions = false\nsmall_model_routing = false\n[agent_team]\nmax_concurrent = 1\nmax_queued = 2\ntoken_budget = 45\n")
File.chmod(0o600, config)
env = { "WILLDEEP_HOME" => home, "WILLDEEP_CONFIG" => config, "WILLDEEP_TELEMETRY_DISABLED" => "1" }
run = lambda do |*args|
  _, _, status = Open3.capture3(env, binary, *args)
  raise "fixture CLI failed: #{args.join(' ')}" unless status.success?
end
api = lambda do |operation, params = {}|
  state = JSON.parse(File.read(File.join(home, "runtime/daemon.json")))
  host, port = state.fetch("address").split(":")
  http = Net::HTTP.new(host, Integer(port), nil)
  http.read_timeout = 60
  request = Net::HTTP::Post.new("/v1/api")
  request["x-willdeep-token"] = state.fetch("token")
  request["content-type"] = "application/json"
  request.body = JSON.generate(protocol_version: "1.0", request_id: SecureRandom.uuid, operation: operation, params: params)
  value = JSON.parse(http.request(request).body)
  raise "fixture API #{operation} rejected: #{value.dig('error', 'message')}" unless value["status"] == "ok"
  value.fetch("data")
end
wait = lambda do |&condition|
  deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + 60
  until condition.call
    raise "fixture condition timed out" if Process.clock_gettime(Process::CLOCK_MONOTONIC) >= deadline
    sleep 0.05
  end
end
checks = []
begin
  run.call("daemon", "start")
  session = api.call("session.create", { id: nil, workspace: workspace, profile: nil, model: "fixture", title: "Team fixture" })
  run.call("daemon", "stop")
  child = SecureRandom.uuid
  root = session.fetch("root_agent_id")
  agent = { id: child, parent_id: root, task_id: SecureRandom.uuid, label: "Fixture child", background: true,
    workspace: workspace, root_workspace: workspace, profile: "generalist", model: "fixture", status: "completed",
    current_turn: 0, current_tool: nil, input_tokens: nil, output_tokens: nil, total_tokens: nil,
    created_at: Time.now.to_i, updated_at: Time.now.to_i, completed_at: Time.now.to_i, error: nil }
  File.write(File.join(home, "runtime/agents.json"), JSON.generate([agent, agent.merge(id: root, parent_id: nil, background: false)]))
  dispatches = File.join(home, "workers/dispatches")
  FileUtils.mkdir_p(dispatches)
  dispatch = { version: 1, id: child, parent_session: session.fetch("id"), args: { profile: "generalist", prompt: "original fixture", run_in_background: true },
    approved_targets: nil, approved_command: nil, model: "fixture", prepared: { workspace: workspace, root_workspace: workspace, branch: nil, dedicated: false } }
  File.write(File.join(dispatches, "#{child}.json"), JSON.generate(dispatch))
  run.call("daemon", "start")
  receipts = []
  4.times do |index|
    if index == 2
      run.call("daemon", "stop")
      run.call("daemon", "start")
    end
    params = { id: child, message: "fixture followup #{index + 1}", request_id: SecureRandom.uuid }
    receipt = api.call("agent.continue", params)
    raise "duplicate continuation changed receipt" unless api.call("agent.continue", params) == receipt
    receipts << receipt
    wait.call { api.call("agent.runs", { id: child, after_sequence: nil, limit: 64 }).fetch("runs").any? { |item| item["id"] == receipt["run_id"] && !%w[queued running waiting_approval waiting_answer].include?(item["status"]) } }
    expected_turn_status = index == 3 ? "partial" : "completed"
    wait.call { api.call("turn.get", { id: receipt["turn_id"] }).fetch("status") == expected_turn_status }
  end
  runs = api.call("agent.runs", { id: child, after_sequence: nil, limit: 64 }).fetch("runs")
  raise "budget admission failed" unless runs.map { |item| item["status"] } == %w[completed completed completed blocked]
  raise "parent or duplicate requested a model" unless mutex.synchronize { calls.length } == 3
  raise "continuation lost previous task history" unless mutex.synchronize { JSON.generate(calls[1]).include?("fixture followup 1") && JSON.generate(calls[1]).include?("fixture followup 2") }
  summary = api.call("agent.team", { id: child })
  raise "budget or usage reset across restart" unless summary.dig("usage", "accounted_tokens") == 45 && summary.dig("child_usage", "model_calls") == 3
  raise "automatic statistics exposed worker report" unless summary.fetch("workers").all? { |worker| worker.dig("latest_run", "report").nil? }
  first = api.call("agent.runs", { id: child, after_sequence: nil, limit: 1 })
  second = api.call("agent.runs", { id: child, after_sequence: first.fetch("next_after_sequence"), limit: 1 })
  raise "pagination lost Run identity" unless first.fetch("runs")[0]["id"] != second.fetch("runs")[0]["id"]
  parent = JSON.parse(File.read(File.join(home, "sessions", "#{session.fetch('id')}.json")))
  raise "report receipts were not committed" unless receipts.all? { |receipt| parent.fetch("agent_reports_received").include?(receipt["run_id"]) }
  checks = ["空闲父会话续做同一 Agent，保留历史", "相同请求只创建一个 Run，主模型零额外调用", "重启后累计预算保持，预算用尽的后续 Run 明确 Blocked", "分页历史与报告回执持久化，自动统计不包含报告"]
  report = { version: "0.99.0-rc1", status: "passed", binary_sha256: Digest::SHA256.file(binary).hexdigest, model_calls: 3, checks: checks }
  File.write(File.join(__dir__, "runtime-team-smoke.json"), JSON.pretty_generate(report) + "\n")
  File.write(File.join(__dir__, "runtime-team-smoke.md"), "# 多 Agent 原生 Runtime 验收\n\n隔离 home、本机假 Provider；没有使用真实模型或用户凭据。\n\n" + checks.map { |check| "- 通过：#{check}" }.join("\n") + "\n")
  puts JSON.generate(status: "passed", checks: checks.length, model_calls: 3)
  if ENV["WILLDEEP_VERIFY_WEB"]
    web = api.call("webapp.start", { listen: "127.0.0.1:0", workspace: workspace, config: config, profile: nil, language: "zh-CN" })
    File.write(ENV.fetch("WILLDEEP_VERIFY_WEB"), JSON.generate(address: web.fetch("listen"), home: home, workspace: workspace, config: config, session_id: session.fetch("id"), agent_id: child))
    deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + 180
    sleep 0.1 until File.exist?(ENV.fetch("WILLDEEP_VERIFY_WEB") + ".done") || Process.clock_gettime(Process::CLOCK_MONOTONIC) >= deadline
  end
ensure
  if $!
    diagnostics = Dir.glob(File.join(home, "workers/team-runs/*/*.json")).map { |file| JSON.parse(File.read(file)).fetch("record").slice("sequence", "status", "usage", "report") }
    warn JSON.generate(runs: diagnostics, model_calls: mutex.synchronize { calls.length })
  end
  run.call("daemon", "stop") rescue nil
  if $!
    log = File.join(home, "runtime/daemon.log")
    warn File.read(log).lines.grep(/Runtime control API internal error/).last(3).join if File.file?(log)
  end
  provider.close
  provider_thread.join(5)
  FileUtils.remove_entry(home) if File.directory?(home)
end
