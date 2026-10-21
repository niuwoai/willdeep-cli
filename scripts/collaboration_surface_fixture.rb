#!/usr/bin/env ruby
# Temporary real Web/TUI server with synthetic sessions, isolated from user data.
require_relative 'verify_session_collaboration'
require 'net/http'

class SurfaceModelFixture < ModelFixture
  def completion(request)
    result = super
    message = result.dig(:choices, 0, :message)
    if message && message[:content] == 'Fixture finished within the authorized task.'
      message[:content] = "我会先从登录流程中最容易打断用户的地方入手。\n\n### 三个优先改进的方向\n\n1. **错误反馈更明确**：在对应字段附近提示问题，保留已输入内容，避免用户重复填写。\n2. **键盘操作更顺畅**：统一焦点顺序，支持 Enter 提交，让校验结果能被读屏识别。\n3. **窄屏布局更从容**：保持单列结构，为输入框和主要按钮留出足够空间。\n\n回归测试会话已同步检查结果。下一步可以对照具体变更逐项审阅，再决定是否集成。\n\n> 本页内容由本地合成数据生成，仅用于界面验收。"
    end
    result
  end
end
fixture = SurfaceModelFixture.new
Dir.mktmpdir('wd-ui-', '/private/tmp') do |root|
  home = File.join(root, 'home')
  workspace = File.join(root, 'workspace')
  FileUtils.mkdir_p([home, workspace], mode: 0o700)
  File.write(File.join(home, 'config.toml'), "version=1\ndefault_provider=\"fixture\"\n[agent]\nmax_turns=8\nsafety_judge=false\n[local_model]\nenabled=false\n[providers.fixture]\nprovider=\"openai-compatible\"\napi=\"chat-completions\"\napi_base=\"http://127.0.0.1:#{fixture.port}/v1\"\napi_key=\"fixture-only\"\nmodel=\"fixture-model\"\n", perm: 0o600)
  env = { 'WILLDEEP_HOME' => home, 'WILLDEEP_CONFIG' => File.join(home, 'config.toml'),
    'WILLDEEP_API_KEY' => nil, 'WILLDEEP_API_BASE' => nil, 'WILLDEEP_MODEL' => nil }
  log = File.open(File.join(root, 'web.log'), 'w')
  pid = nil
  begin
    call_api(env, 'workspace.register', root: workspace, name: 'WillDeep 演示项目', provider_profile: 'fixture')
    fixture.source = call_api(env, 'session.create', workspace: workspace, title: '登录体验优化', profile: 'fixture').fetch('id')
    fixture.target = call_api(env, 'session.create', workspace: workspace, title: '回归测试与验收', profile: 'fixture').fetch('id')
    call_api(env, 'session.peer.send', source_session_id: fixture.target, session_id: fixture.source, message: '登录表单的回归检查已完成。错误反馈和键盘操作均已覆盖，窄屏布局还需要一起确认。请保留当前提交，等待统一集成。')
    [fixture.source, fixture.target].each do |id|
      call_api(env, 'turn.submit', session_id: id, turn_request_id: SecureRandom.uuid,
        prompt: '请梳理登录页的改进方案，重点检查错误反馈、键盘操作和窄屏布局。此为合成演示，仅检查界面，不修改项目文件。', origin_client: 'fixture-operator')
    end
    socket = TCPServer.new('127.0.0.1', Integer(ENV.fetch('WILLDEEP_SURFACE_PORT', '0')))
    port = socket.addr[1]
    socket.close
    pid = Process.spawn(env, BIN, '--web', '--listen', "127.0.0.1:#{port}", '-w', workspace, '-c', File.join(home, 'config.toml'), '--language', 'zh-CN', out: log, err: log)
    url = "http://127.0.0.1:#{port}"
    wait_for('Web listener') { (Net::HTTP.get_response(URI("#{url}/api/sessions")).code == '200' rescue false) }
    ready = { root: root, home: home, workspace: workspace, source: fixture.source, target: fixture.target, url: url,
      done: File.join(root, 'done') }
    File.write('/private/tmp/wd-collaboration-surface-ready.json', JSON.pretty_generate(ready), perm: 0o600)
    puts JSON.generate(ready)
    $stdout.flush
    Timeout.timeout(3600) { sleep 0.2 until File.exist?(ready[:done]) }
    notes = call_api(env, 'session.peer.inbox', session_id: fixture.source)
    FileUtils.mkdir_p(File.dirname(REPORT))
    File.write(File.join(File.dirname(REPORT), 'surface-inbox.json'), JSON.pretty_generate(notes))
  ensure
    Process.kill('TERM', pid) if pid
    Process.wait(pid) if pid
    log.close
    Open3.capture3(env, BIN, 'daemon', 'stop')
  end
end
fixture.close
