require "json"
require "net/http"
require "open3"
require "securerandom"
require "tmpdir"
require "fileutils"
require "digest"

binary = File.expand_path(ARGV.fetch(0))
home = Dir.mktmpdir("willdeep-web-runtime-", "/private/tmp")
workspace = File.join(home, "workspace")
FileUtils.mkdir_p(workspace)
config = File.join(home, "config.toml")
File.write(config, "version = 1\ndefault_provider = \"fixture\"\n[providers.fixture]\nprovider = \"openai-compatible\"\napi_base = \"http://127.0.0.1:1/v1\"\napi_key = \"fixture-not-a-real-key\"\nmodel = \"fixture\"\n[agent]\nauto_title = false\ninput_suggestions = false\nsmall_model_routing = false\n")
File.chmod(0o600, config)
env = { "WILLDEEP_HOME" => home, "WILLDEEP_CONFIG" => config, "WILLDEEP_TELEMETRY_DISABLED" => "1" }
state_path = File.join(home, "runtime/daemon.json")
checks = []

run = lambda do |*arguments|
  output, error, status = Open3.capture3(env, binary, *arguments)
  raise "fixture CLI command failed: #{arguments.join(' ')}" unless status.success?
  output
end
wait = lambda do |&condition|
  deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + 40
  until condition.call
    raise "fixture condition timed out" if Process.clock_gettime(Process::CLOCK_MONOTONIC) >= deadline
    sleep 0.05
  end
end
state = -> { JSON.parse(File.read(state_path)) }
api = lambda do |operation, params = {}|
  current = state.call
  host, port = current.fetch("address").split(":")
  http = Net::HTTP.new(host, Integer(port), nil)
  http.open_timeout = 5
  http.read_timeout = 40
  request = Net::HTTP::Post.new("/v1/api")
  request["x-willdeep-token"] = current.fetch("token")
  request["content-type"] = "application/json"
  request.body = JSON.generate(protocol_version: "1.0", request_id: SecureRandom.uuid, operation: operation, params: params)
  response = http.request(request)
  value = JSON.parse(response.body)
  raise "fixture API #{operation} failed" unless response.is_a?(Net::HTTPSuccess) && value["status"] == "ok"
  value.fetch("data")
end
health = lambda do |address|
  host, port = address.split(":")
  Net::HTTP.new(host, Integer(port), nil).get("/health").code == "200"
rescue SystemCallError, IOError, Net::OpenTimeout, Net::ReadTimeout
  false
end

begin
  run.call("daemon", "start") # The native CLI exits while its Runtime stays alive.
  initial_pid = state.call.fetch("pid")
  started = api.call("webapp.start", { listen: "127.0.0.1:0", workspace: workspace, config: config, profile: nil, language: "zh-CN" })
  address = started.fetch("listen")
  raise "fixture Web service is not running" unless started["running"] && health.call(address)
  checks << { name: "CLI 启动进程退出后，Runtime 托管的 Web 仍可访问", status: "passed" }

  # Make an actual short-lived API client process and verify the listener after exit.
  client = "require 'json'; require 'net/http'; s=JSON.parse(File.read(ARGV[0])); h,p=s.fetch('address').split(':'); q=Net::HTTP::Post.new('/v1/api'); q['x-willdeep-token']=s.fetch('token'); q['content-type']='application/json'; q.body=STDIN.read; r=Net::HTTP.new(h,Integer(p),nil).request(q); exit(r.code=='200' ? 0 : 1)"
  body = JSON.generate(protocol_version: "1.0", request_id: SecureRandom.uuid, operation: "webapp.status", params: {})
  _, _, result = Open3.capture3(RbConfig.ruby, "-e", client, state_path, stdin_data: body)
  raise "fixture API client failed" unless result.success?
  raise "Web listener disappeared with client" unless health.call(address) && state.call.fetch("pid") == initial_pid
  checks << { name: "独立 API 客户端退出不关闭 Web 监听", status: "passed" }

  run.call("daemon", "stop")
  wait.call { !health.call(address) }
  raise "Runtime stop erased enabled intent" unless JSON.parse(File.read(File.join(home, "webapp-service.json")))["enabled"]
  run.call("daemon", "start")
  wait.call { health.call(address) }
  restored = api.call("webapp.status")
  raise "Web address was not restored" unless restored["listen"] == address && restored["running"]
  checks << { name: "Runtime 停止关闭 Web，重启恢复原监听地址", status: "passed" }

  stopped = api.call("webapp.stop")
  raise "Web disable did not persist" if stopped["enabled"] || stopped["running"]
  wait.call { !health.call(address) }
  run.call("daemon", "stop")
  run.call("daemon", "start")
  final = api.call("webapp.status")
  raise "Explicitly disabled Web was restored" if final["enabled"] || final["running"] || health.call(address)
  checks << { name: "显式关闭 Web 的开关跨 Runtime 重启保持关闭", status: "passed" }

  report = { version: "0.99.0-rc1", status: "passed", binary_sha256: Digest::SHA256.file(binary).hexdigest, checks: checks, scope: "isolated fixture Runtime; no model calls; installed user Runtime untouched" }
  File.write(File.join(__dir__, "runtime-web-smoke.json"), JSON.pretty_generate(report) + "\n")
  File.write(File.join(__dir__, "runtime-web-smoke.md"), "# Runtime 托管 Web 进程验收\n\n版本：0.99.0-rc1。独立临时 home、工作区和假 Provider；没有调用模型，也没有停止或升级用户原有 Runtime。\n\n" + checks.map { |item| "- 通过：#{item[:name]}" }.join("\n") + "\n\n构建产物 SHA-256 与逐项结果见同名 JSON。\n")
  puts JSON.generate(status: "passed", checks: checks.length)
ensure
  run.call("daemon", "stop") rescue nil
  FileUtils.remove_entry(home) if File.directory?(home)
end
