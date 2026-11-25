# frozen_string_literal: true

require "fileutils"
require "net/http"
require "open3"
require "securerandom"
require "timeout"
require "tmpdir"
require "uri"
require_relative "../scripts/test/tui_interaction_fixture"

class DesktopService
  attr_reader :boot, :cookie

  def initialize(environment, binary)
    @input, @output, @errors, @process = Open3.popen3(environment, binary, "desktop-serve")
    @boot = Timeout.timeout(45) do
      @output.each_line do |line|
        next unless line.start_with?('{"willdeep_desktop_url":')

        break URI(JSON.parse(line).fetch("willdeep_desktop_url"))
      end
    end
    raise "Desktop service ended before reporting its address" unless @boot.is_a?(URI::HTTP)

    @output_reader = Thread.new { @output.read }
    @error_reader = Thread.new { @errors.read }
  rescue StandardError
    close
    raise
  end

  def authorize
    result = request(:get, boot.path, authenticate: false)
    raise "Desktop bootstrap returned #{result.code}" unless result.code == "200"
    unless result.content_type == "text/html" && result["location"].nil? &&
           result.body.include?('http-equiv="refresh" content="0; url=/?desktop=1"')
      raise "Desktop bootstrap must navigate from a committed local document"
    end
    raise "Desktop cookie flags missing" unless result["set-cookie"]&.match?(/HttpOnly; SameSite=Strict/)

    @cookie = result["set-cookie"].split(";").first
  end

  def request(method, path, body: nil, authenticate: true, headers: {}, &block)
    http = Net::HTTP.new(boot.host, boot.port, nil)
    http.open_timeout = 5
    http.read_timeout = 30
    args = { "Origin" => "http://#{boot.host}:#{boot.port}" }.merge(headers)
    args["Cookie"] = cookie if authenticate && cookie
    request_class = { get: Net::HTTP::Get, post: Net::HTTP::Post, put: Net::HTTP::Put }.fetch(method)
    request = request_class.new(path, args)
    if body
      request["Content-Type"] = "application/json"
      request.body = JSON.generate(body)
    end
    http.start { |connection| connection.request(request, &block) }
  end

  def stream(method, path, body = nil)
    queue = Queue.new
    reader = Thread.new do
      request(method, path, body: body) do |response|
        raise "Desktop stream returned #{response.code}" unless response.code == "200"

        buffer = +""
        response.read_body do |fragment|
          buffer << fragment
          while (line = buffer.slice!(/\A[^\n]*\n/))
            queue << JSON.parse(line.delete_prefix("data:").strip) if line.start_with?("data:")
          end
        end
      end
    rescue StandardError => error
      queue << error
    end
    [queue, reader]
  end

  def close
    @input&.close unless @input&.closed?
    if @process && !@process.join(5)
      Process.kill("TERM", @process.pid)
      @process.join(5)
    end
    @output&.close unless @output&.closed?
    @errors&.close unless @errors&.closed?
    @output_reader&.join(1)
    @error_reader&.join(1)
  end
end

def require_evidence(condition, label)
  raise label unless condition
end

def query(path, **params)
  "#{path}?#{URI.encode_www_form(params)}"
end

def json_response(service, method, path, code = "200", body: nil)
  response = service.request(method, path, body: body)
  require_evidence(response.code == code, "#{method} #{path.split('?').first} returned #{response.code}")
  JSON.parse(response.body) unless code == "204"
end

def until_found(description)
  Timeout.timeout(25) do
    loop do
      value = yield
      return value if value

      sleep 0.1
    end
  end
rescue Timeout::Error
  raise "Timed out waiting for #{description}"
end

def event(queue, type)
  until_found("desktop event #{type}") do
    next if queue.empty?

    value = queue.pop
    raise value if value.is_a?(Exception)
    raise "Desktop streamed a failed turn" if %w[error failed].include?(value["type"])

    value if value["type"] == type
  end
end

def shell!(*argv, chdir:)
  require_evidence(system(*argv, chdir: chdir, out: File::NULL, err: File::NULL), "Fixture #{argv.take(2).join(' ')} failed")
end

def configure(root, provider)
  home = File.join(root, "runtime-home")
  user_home = File.join(root, "fixture-user")
  FileUtils.mkdir_p(home, mode: 0o700)
  FileUtils.mkdir_p(user_home, mode: 0o700)
  File.write(File.join(home, "config.toml"), <<~TOML, perm: 0o600)
    version = 1
    default_provider = "fixture"
    [agent]
    approval = "strict"
    auto_title = false
    input_suggestions = false
    auto_dispatch_read_only = false
    [telemetry]
    enabled = false
    [providers.fixture]
    provider = "openai-compatible"
    api = "chat-completions"
    api_base = "http://127.0.0.1:#{provider.port}/v1"
    api_key = "desktop-local-fixture"
    model = "fixture"
  TOML
  { "HOME" => user_home, "USERPROFILE" => user_home, "WILLDEEP_HOME" => home,
    "WILLDEEP_CONFIG" => File.join(home, "config.toml"),
    "WILLDEEP_TELEMETRY_DISABLED" => "1", "NO_PROXY" => "127.0.0.1,localhost" }
end

def workspace_fixture(root)
  worktree = File.join(root, "workspace")
  FileUtils.mkdir_p(worktree)
  shell!("git", "init", chdir: worktree)
  path = File.join(worktree, "existing.txt")
  File.write(path, "base\n")
  File.write(File.join(worktree, "test"), 'abort unless File.read("fixture-note.txt") == "Desktop approved text\n"' + "\n")
  shell!("git", "add", "existing.txt", "test", chdir: worktree)
  shell!("git", "-c", "user.name=Desktop Fixture", "-c", "user.email=fixture@example.invalid",
         "commit", "-m", "Baseline", chdir: worktree)
  File.write(path, "workspace-only adjustment\n")
  worktree
end

def validate_access(service, checks)
  %w[/ /health /api/workspaces /api/desktop/runtime].each do |path|
    require_evidence(service.request(:get, path, authenticate: false).code == "401", "Private desktop endpoint accepted unauthenticated requests")
  end
  service.authorize
  require_evidence(service.request(:get, service.boot.path).code == "404", "Launch URL was accepted twice")
  require_evidence(service.request(:get, "/health", headers: { "Origin" => "https://example.invalid" }).code == "403", "Foreign origin passed authentication")
  require_evidence(service.request(:get, "/health", headers: { "Host" => "example.invalid" }).code == "403", "Foreign host passed authentication")
  require_evidence(service.request(:get, "/?desktop=1").body.include?("/assets/"), "Packaged React HTML missing")
  status = json_response(service, :get, "/api/desktop/runtime")
  require_evidence(status["desktop_version"] == status["runtime_version"], "Bundled runtime and Web versions differ")
  checks << "一次性启动、本地来源和未登录拒绝"
end

def desktop_connected?(environment)
  path = File.join(environment.fetch("WILLDEEP_HOME"), "runtime", "desktop-presence.lock")
  File.open(path, "r+") { |handle| !handle.flock(File::LOCK_EX | File::LOCK_NB) }
end

def validate_changes(service, workspace, session, checks)
  path = query("/api/desktop/changes", workspace: workspace, session: session)
  changes = json_response(service, :get, path)
  snapshot = changes.fetch("snapshot")
  require_evidence(snapshot.fetch("files").map { |file| file["path"] }.sort == %w[existing.txt fixture-note.txt], "Diff file list lost workspace changes")
  require_evidence(changes.fetch("session_paths") == ["fixture-note.txt"], "Diff misattributed existing workspace changes")
  diff_path = query("/api/desktop/changes/file", workspace: workspace, session: session,
                    snapshot_id: snapshot.fetch("id"), path: "fixture-note.txt")
  require_evidence(json_response(service, :get, diff_path).fetch("content").include?("+Desktop approved text"), "File Diff lost approved text")
  checks << "逐文件 Diff 与本会话归属"
end

def verify_parallel_workspace(service, provider, root, primary_activity, primary_gate)
  plain = File.join(root, "non-git")
  FileUtils.mkdir_p(plain)
  json_response(service, :post, "/api/workspaces", body: { path: plain })
  stream, reader = service.stream(:post, "/api/chat/stream", {
    workspace: plain, prompt: "Complete the independent workspace task.", request_id: SecureRandom.uuid, language: "en"
  })
  submitted = event(stream, "submitted")
  until_found("the independent workspace Provider request") { provider.requests[1] }
  provider.delta(1, "Independent workspace result.")
  provider.finish(1)
  event(stream, "completed")
  reader.join(5)
  session = submitted.fetch("session_id")
  detail = json_response(service, :get, "/api/sessions/#{session}")
  require_evidence(detail["workspace"] == plain, "Parallel turn crossed the workspace boundary")
  require_evidence(JSON.generate(detail["messages"]).include?("Independent workspace result."), "Independent workspace history was lost")
  require_evidence(json_response(service, :get, primary_activity).fetch("gates").any? { |gate| gate["id"] == primary_gate["id"] },
                   "Parallel turn changed approval in the primary workspace")
  plain
end

def allow_once(service, gate, workspace, session)
  json_response(service, :post, "/api/runtime/approvals/#{gate.fetch('id')}/resolve", "204",
                body: { workspace: workspace, session: session, decision: "allow_once" })
end

def verify_approved_file(service, provider, activity, previous_gate, workspace, session)
  provider.tool(2, "run_command", { command: "ruby test" })
  verification = until_found("the verification command or its approval") do
    next :continued if provider.requests[3]

    json_response(service, :get, activity).fetch("gates").find do |gate|
      gate["kind"] == "approval" && gate["id"] != previous_gate["id"]
    end
  end
  allow_once(service, verification, workspace, session) unless verification == :continued
  until_found("the Provider request after verification") { provider.requests[3] }
end

def verify(binary, root, checks)
  provider = InteractionProvider.new
  environment = configure(root, provider)
  workspace = workspace_fixture(root)
  service = DesktopService.new(environment, binary)
  validate_access(service, checks)
  require_evidence(desktop_connected?(environment), "Open desktop did not hold Runtime presence")
  json_response(service, :post, "/api/workspaces", body: { path: workspace })
  json_response(service, :post, "/api/workspaces/approval-mode", body: { path: workspace, approval_mode: "strict" })
  stream, first_reader = service.stream(:post, "/api/chat/stream", {
    workspace: workspace, prompt: "Create the approved fixture note, then verify it with ruby test.", request_id: SecureRandom.uuid, language: "en"
  })
  submitted = event(stream, "submitted")
  session = submitted.fetch("session_id")
  json_response(service, :put, "/api/desktop/selection", "204", body: { workspace: workspace, session: session })
  until_found("the initial Provider request") { provider.requests.first }
  provider.tool(0, "create_file", { path: "fixture-note.txt", content: "Desktop approved text\n" })
  activity = query("/api/runtime/activity", workspace: workspace, session: session)
  gate = until_found("the pending approval") { json_response(service, :get, activity).fetch("gates").find { |value| value["kind"] == "approval" } }
  plain = verify_parallel_workspace(service, provider, root, activity, gate)
  checks << "两个工作区并行执行与审批隔离"
  daemon_path = File.join(environment.fetch("WILLDEEP_HOME"), "runtime", "daemon.json")
  daemon = JSON.parse(File.read(daemon_path)).fetch("pid")
  require_evidence(!File.exist?(File.join(workspace, "fixture-note.txt")), "Tool wrote before approval")
  service.close
  require_evidence(!desktop_connected?(environment), "Closed desktop left the Runtime presence held")
  first_reader.join(5)
  service = DesktopService.new(environment, binary)
  service.authorize
  require_evidence(desktop_connected?(environment), "Reopened desktop did not hold Runtime presence")
  require_evidence(JSON.parse(File.read(daemon_path)).fetch("pid") == daemon, "Closing desktop disconnected its daemon")
  require_evidence(json_response(service, :get, "/api/desktop/selection").fetch("session") == session, "Desktop selection did not survive restart")
  require_evidence(json_response(service, :get, activity).fetch("gates").any? { |value| value["id"] == gate["id"] }, "Approval was lost on restart")
  checks << "关窗后保留 Runtime、会话和待审批"

  resume = query("/api/sessions/#{session}/stream", after: submitted.fetch("cursor"), language: "en")
  resumed, second_reader = service.stream(:get, resume)
  require_evidence(event(resumed, "resumed").fetch("turn_id") == submitted.fetch("turn_id"), "Desktop resumed a different turn")
  allow_once(service, gate, workspace, session)
  until_found("the Provider request after approval") { provider.requests[2] }
  verify_approved_file(service, provider, activity, gate, workspace, session)
  provider.delta(3, "Desktop task completed.")
  provider.finish(3)
  event(resumed, "completed")
  second_reader.join(5)
  detail = json_response(service, :get, "/api/sessions/#{session}")
  require_evidence(JSON.generate(detail.fetch("messages")).include?("Desktop task completed."), "Completed reply was lost from history")
  checks << "按游标续接、批准执行与历史回读"
  validate_changes(service, workspace, session, checks)
  require_evidence(json_response(service, :get, query("/api/desktop/changes", workspace: plain))["git"] == false,
                   "Non-Git folder did not get a normal empty view")
  foreign = query("/api/desktop/changes", workspace: plain, session: session)
  require_evidence(service.request(:get, foreign).code == "403", "Diff accepted a mismatched workspace and session")
  checks << "普通目录与跨工作区隔离"
ensure
  service&.close
  provider&.close
  system(environment, binary, "daemon", "stop", out: File::NULL, err: File::NULL) if environment
end

if $PROGRAM_NAME == __FILE__
  binary = File.realpath(ARGV.fetch(0))
  output = File.expand_path(ARGV.fetch(1, "../.build/desktop/verification"), __dir__)
  FileUtils.mkdir_p(output)
  checks = []
  failure = nil
  begin
    Dir.mktmpdir("wd-desktop-", "/tmp") { |root| verify(binary, File.realpath(root), checks) }
  rescue StandardError => error
    failure = "#{error.class}: #{error.message}"
  ensure
    result = { passed: failure.nil?, checks: checks, failure: failure }
    File.write(File.join(output, "smoke.json"), JSON.pretty_generate(result) + "\n")
    lines = ["# WillDeep Desktop 端到端验收", "", *checks.map { |label| "- 通过 · #{label}" }]
    lines << "- 失败 · #{failure}" if failure
    File.write(File.join(output, "smoke.md"), lines.join("\n") + "\n")
    puts lines.join("\n")
  end
  exit(failure ? 1 : 0)
end
