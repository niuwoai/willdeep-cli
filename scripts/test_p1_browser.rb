#!/usr/bin/env ruby
# Real Node/CDP protocol acceptance against the already-running local fixture.
# No package installation, external requests, user profile or Provider environment.
require 'base64'
require 'digest'
require 'fileutils'
require 'json'
require 'open3'
require 'optparse'
require 'time'
require 'timeout'
require 'tmpdir'
require 'uri'

ROOT = File.expand_path('..', __dir__)
VERSION = '0.151.0-rc1'
BASE = 'http://127.0.0.1:19851'
options = {
  node: '/opt/homebrew/bin/node',
  chrome: '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
  report_prefix: File.join(ROOT, 'docs/verification', VERSION, 'browser-bridge')
}
OptionParser.new do |parser|
  parser.on('--node PATH') { |path| options[:node] = path }
  parser.on('--chrome PATH') { |path| options[:chrome] = path }
  parser.on('--report-prefix PATH') { |path| options[:report_prefix] = path }
end.parse!
[options[:node], options[:chrome]].each do |path|
  abort 'Explicit absolute executable path required' unless path.start_with?('/') && File.executable?(path)
end
report = {
  version: VERSION, suite: 'browser-bridge-protocol', started_at: Time.now.utc.iso8601,
  fixture_origin: BASE, tests: [],
  boundaries: [
    'Real Node/Chrome CDP bridge; Rust host and shared execute/approval not compiled or executed by this script.',
    'Navigation/request origin controls and Chrome background flags; not whole-process network isolation.',
    'One disposable profile per process, clean environment; no dependency installation or external service.'
  ],
  not_verified: ['Rust compilation/unit tests', 'Runtime approval and host ref binding', 'Windows process job', 'OS-level network isolation']
}

def check(report, name)
  start = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  detail = yield
  report[:tests] << { name: name, status: 'passed', seconds: (Process.clock_gettime(Process::CLOCK_MONOTONIC) - start).round(3), detail: detail }
rescue StandardError => exception
  report[:tests] << { name: name, status: 'failed', error: exception.message }
  raise
end

def assert(condition, message)
  raise message unless condition
end

Dir.mktmpdir('willdeep-browser-acceptance-') do |scratch|
  env = { 'HOME' => scratch, 'TMPDIR' => scratch, 'LANG' => 'C.UTF-8' }
  bridge = File.join(ROOT, 'crates/willdeep-core/src/tools/browser/bridge.mjs')
  stdin, stdout, stderr, wait = Open3.popen3(env, options[:node], bridge, unsetenv_others: true, pgroup: true, chdir: scratch)
  sequence = 0
  request = lambda do |args = nil, success: true, **fields|
    args ||= fields
    sequence += 1
    stdin.puts(JSON.generate(args.merge(id: sequence)))
    stdin.flush
    line = Timeout.timeout(30) { stdout.gets }
    raise 'Bridge terminated without response' unless line
    raise 'Bridge response exceeded budget' if line.bytesize > 12 * 1024 * 1024
    response = JSON.parse(line)
    assert(response['id'] == sequence, 'Bridge response ID mismatch')
    assert(response['ok'] == success, "Bridge result mismatch: #{response['error']}")
    success ? response.fetch('result') : response.fetch('error')
  end
  current = nil
  snapshot = nil
  begin
    check(report, 'isolated_profile_clean_environment_and_dependencies') do
      current = request.call(action: 'init', chrome_path: options[:chrome], profile: File.join(scratch, 'profile'), allowed_origins: [BASE])
      assert(current['profile_isolated'], 'Profile isolation was not confirmed')
      assert(current['node_version'].split('.').first.to_i >= 24, 'Node is older than 24')
      assert(File.directory?(File.join(scratch, 'profile')), 'No fresh profile')
      { node_version: current['node_version'], chrome_version: current['chrome_version'], profile_isolated: true, clean_environment: true }
    end
    check(report, 'open_and_snapshot_local_form') do
      current = request.call(action: 'open', expected_epoch: current['epoch'], url: "#{BASE}/")
      snapshot = request.call(action: 'snapshot', expected_epoch: current['epoch'])
      assert(snapshot['text'].include?('P1 local fixture'), 'Missing local page text')
      assert(snapshot['nodes'].any? { |node| node['id'] == 'name' }, 'Missing input node')
      { epoch: snapshot['epoch'], node_count: snapshot['nodes'].length, http_status: current['http_status'] }
    end
    check(report, 'fill_submit_and_business_assertion') do
      input = snapshot['nodes'].find { |node| node['id'] == 'name' }
      current = request.call(action: 'fill', expected_epoch: snapshot['epoch'], backend_node_id: input['backend_node_id'], text: 'P1 synthetic browser item')
      snapshot = request.call(action: 'snapshot', expected_epoch: current['epoch'])
      save = snapshot['nodes'].find { |node| node['id'] == 'save' }
      current = request.call(action: 'click', expected_epoch: snapshot['epoch'], backend_node_id: save['backend_node_id'])
      # Wait for an asynchronous local POST by reading snapshots only. Input is never replayed.
      50.times do
        snapshot = request.call(action: 'snapshot', expected_epoch: current['epoch'])
        break if snapshot['text'].include?('Saved')
        current = request.call(action: 'status')
      end
      status = snapshot['nodes'].find { |node| node['id'] == 'status' }
      result = request.call(action: 'assert', expected_epoch: snapshot['epoch'], backend_node_id: status['backend_node_id'], assertion: 'text_equals', expected: 'Saved')
      assert(result['passed'], 'Business status assertion did not pass')
      assert(snapshot['text'].include?('P1 synthetic browser item'), 'Saved item missing')
      { form: 'submitted once', assertion: 'status text equals Saved', passed: true }
    end
    check(report, 'screenshot_png_hash_mime_and_dimensions') do
      current = request.call(action: 'status')
      result = request.call(action: 'screenshot', expected_epoch: current['epoch'])
      png = Base64.strict_decode64(result.fetch('png'))
      assert(png.start_with?("\x89PNG\r\n\x1a\n".b), 'PNG signature mismatch')
      width, height = png.byteslice(16, 8).unpack('NN')
      assert(width == 1280 && height == 720, 'Unexpected viewport dimensions')
      { mime: 'image/png', width: width, height: height, bytes: png.bytesize, sha256: Digest::SHA256.hexdigest(png), persistence: 'protocol bytes only; Rust host owns file path' }
    end
    check(report, 'route_navigation_and_stale_epoch_refusal') do
      current = request.call(action: 'status')
      snapshot = request.call(action: 'snapshot', expected_epoch: current['epoch'])
      old_epoch = snapshot['epoch']
      link = snapshot['nodes'].find { |node| node['id'] == 'next' }
      current = request.call(action: 'click', expected_epoch: old_epoch, backend_node_id: link['backend_node_id'])
      assert(current['url'] == "#{BASE}/next", 'Click did not navigate to route')
      assert(current['epoch'] != old_epoch, 'Navigation did not advance page epoch')
      denied = request.call({ action: 'click', expected_epoch: old_epoch, backend_node_id: link['backend_node_id'] }, success: false)
      assert(denied['code'] == 'STALE_PAGE', 'Stale page action was not refused')
      result = request.call(action: 'assert', expected_epoch: current['epoch'], assertion: 'url_equals', expected: "#{BASE}/next")
      assert(result['passed'], 'Route assertion failed')
      { url: current['url'], stale_epoch_code: denied['code'] }
    end
    check(report, 'origin_denial_and_no_arbitrary_actions') do
      current = request.call(action: 'status')
      denied = request.call({ action: 'open', expected_epoch: current['epoch'], url: 'http://127.0.0.1:19852/forbidden' }, success: false)
      assert(denied['code'] == 'ORIGIN_DENIED', 'Unconfigured origin was not refused')
      unsupported = request.call({ action: 'evaluate', expected_epoch: current['epoch'], source: 'document.title' }, success: false)
      assert(unsupported['code'] == 'INVALID_ARGUMENTS', 'Arbitrary JS action was accepted')
      { origin_code: denied['code'], arbitrary_action_code: unsupported['code'] }
    end
    check(report, 'page_world_dom_spoofing_cannot_redirect_approved_click') do
      current = request.call(action: 'status')
      current = request.call(action: 'open', expected_epoch: current['epoch'], url: "#{BASE}/hostile-dom")
      snapshot = request.call(action: 'snapshot', expected_epoch: current['epoch'])
      approved = snapshot['nodes'].find { |node| node['id'] == 'approved' }
      denied = request.call({ action: 'click', expected_epoch: snapshot['epoch'], backend_node_id: approved.fetch('backend_node_id') }, success: false)
      assert(denied['code'] == 'ELEMENT_DENIED', 'Page-world elementFromPoint/contains override bypassed hit testing')
      current = request.call(action: 'status')
      snapshot = request.call(action: 'snapshot', expected_epoch: current['epoch'])
      result = snapshot['nodes'].find { |node| node['id'] == 'result' }
      assertion = request.call(action: 'assert', expected_epoch: snapshot['epoch'], backend_node_id: result.fetch('backend_node_id'), assertion: 'text_equals', expected: 'untouched')
      assert(assertion['passed'], 'A covered target or its overlay received an unapproved click')
      { result: 'ELEMENT_DENIED', target_and_overlay: 'untouched', controller_world: 'isolated' }
    end
    check(report, 'synthetic_application_error_state') do
      current = request.call(action: 'status')
      current = request.call(action: 'open', expected_epoch: current['epoch'], url: "#{BASE}/error")
      snapshot = request.call(action: 'snapshot', expected_epoch: current['epoch'])
      status = snapshot['nodes'].find { |node| node['id'] == 'status' }
      result = request.call(action: 'assert', expected_epoch: snapshot['epoch'], backend_node_id: status['backend_node_id'], assertion: 'text_equals', expected: 'Synthetic error')
      assert(result['passed'], 'Synthetic application error assertion failed')
      { http_status: current['http_status'], application_status: 'Synthetic error', assertion_passed: true }
    end
    check(report, 'close_removes_owned_profile') do
      result = request.call(action: 'close')
      assert(result['closed'], 'Close was not confirmed')
      assert(!File.exist?(File.join(scratch, 'profile')), 'Owned temporary profile remained after close')
      { closed: true, profile_removed: true }
    end
  rescue StandardError => exception
    report[:failure] = exception.message
  ensure
    # Kill the owned group even if the protocol was cancelled or Chrome hung.
    begin
      Process.kill('KILL', -wait.pid)
    rescue Errno::ESRCH
      # The isolated process group already exited.
    end
    stdin.close unless stdin.closed?
    stdout.close unless stdout.closed?
    stderr.close unless stderr.closed?
    wait.join(5)
  end
end
unless report.key?(:failure)
  begin
    check(report, 'driver_cancellation_kills_isolated_node_chrome_process_group') do
      Dir.mktmpdir('willdeep-browser-cancel-') do |scratch|
        bridge = File.join(ROOT, 'crates/willdeep-core/src/tools/browser/bridge.mjs')
        env = { 'HOME' => scratch, 'TMPDIR' => scratch, 'LANG' => 'C.UTF-8' }
        stdin, stdout, stderr, wait = Open3.popen3(env, options[:node], bridge, unsetenv_others: true, pgroup: true, chdir: scratch)
        begin
          stdin.puts(JSON.generate(id: 1, action: 'init', chrome_path: options[:chrome], profile: File.join(scratch, 'profile'), allowed_origins: [BASE]))
          stdin.flush
          response = JSON.parse(Timeout.timeout(30) { stdout.gets } || '{}')
          assert(response['ok'], "Cancellation fixture failed to initialize: #{response['error']}")
          members = lambda do
            output, _, result = Open3.capture3({ 'LC_ALL' => 'C' }, '/bin/ps', '-axo', 'pid=,pgid=,state=', unsetenv_others: true)
            assert(result.success?, 'Cannot inspect owned process group')
            output.lines.filter_map do |line|
              pid, group, state = line.split
              pid.to_i if group.to_i == wait.pid && !state.to_s.start_with?('Z')
            end
          end
          before = members.call
          assert(before.length >= 2, 'Chrome did not share the owned bridge process group')
          Process.kill('KILL', -wait.pid)
          Timeout.timeout(5) { sleep(0.02) until members.call.empty? }
          { owned_group_members_before: before.length, executing_members_after: 0, cancellation: 'driver process-group kill; Rust future cancellation is covered separately by native tests' }
        ensure
          begin
            Process.kill('KILL', -wait.pid)
          rescue Errno::ESRCH
            # Owned group already terminated.
          end
          [stdin, stdout, stderr].each { |stream| stream.close unless stream.closed? }
          wait.join(5)
        end
      end
    end
  rescue StandardError => exception
    report[:failure] = exception.message
  end
end
unless report.key?(:failure)
  begin
    check(report, 'online_status_probes_hung_chrome_and_fails_within_deadline') do
      Dir.mktmpdir('willdeep-browser-hung-') do |scratch|
        bridge = File.join(ROOT, 'crates/willdeep-core/src/tools/browser/bridge.mjs')
        env = { 'HOME' => scratch, 'TMPDIR' => scratch, 'LANG' => 'C.UTF-8' }
        stdin, stdout, stderr, wait = Open3.popen3(env, options[:node], bridge, unsetenv_others: true, pgroup: true, chdir: scratch)
        begin
          stdin.puts(JSON.generate(id: 1, action: 'init', chrome_path: options[:chrome], profile: File.join(scratch, 'profile'), allowed_origins: [BASE]))
          stdin.flush
          response = JSON.parse(Timeout.timeout(30) { stdout.gets } || '{}')
          assert(response['ok'], 'Hung Chrome fixture initialization failed')
          stdin.puts(JSON.generate(id: 2, action: 'status')); stdin.flush
          live = JSON.parse(Timeout.timeout(5) { stdout.gets } || '{}')
          assert(live.dig('result', 'cdp_verified'), 'Online status did not verify CDP')
          output, _, status = Open3.capture3({ 'LC_ALL' => 'C' }, '/bin/ps', '-axo', 'pid=,ppid=', unsetenv_others: true)
          assert(status.success?, 'Cannot inspect owned Chrome process')
          child = output.lines.filter_map { |line| pid, parent = line.split; pid.to_i if parent.to_i == wait.pid }.first
          assert(child, 'No direct owned Chrome process found')
          Process.kill('STOP', child)
          started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
          stdin.puts(JSON.generate(id: 3, action: 'status')); stdin.flush
          failed = JSON.parse(Timeout.timeout(5) { stdout.gets } || '{}')
          elapsed = Process.clock_gettime(Process::CLOCK_MONOTONIC) - started
          assert(failed['ok'] == false && failed.dig('error', 'code') == 'UNKNOWN', 'Hung Chrome was reported ready')
          { live_probe: true, hung_status: 'UNKNOWN', seconds: elapsed.round(3), automatic_retry: false }
        ensure
          begin
            Process.kill('KILL', -wait.pid)
          rescue Errno::ESRCH
            # Owned group already terminated.
          end
          [stdin, stdout, stderr].each { |stream| stream.close unless stream.closed? }
          wait.join(5)
        end
      end
    end
  rescue StandardError => exception
    report[:failure] = exception.message
  end
end
report[:finished_at] = Time.now.utc.iso8601
report[:passed] = !report.key?(:failure) && report[:tests].all? { |test| test[:status] == 'passed' }
FileUtils.mkdir_p(File.dirname(options[:report_prefix]))
File.write("#{options[:report_prefix]}.json", JSON.pretty_generate(report) + "\n")
markdown = ["# Browser bridge #{VERSION}", '', "- Result: #{report[:passed] ? 'passed' : 'failed'}", "- Fixture: #{BASE}", "- Started: #{report[:started_at]}", '', '| Test | Result | Detail |', '| --- | --- | --- |']
report[:tests].each do |test|
  markdown << "| #{test[:name]} | #{test[:status]} | #{JSON.generate(test[:detail] || test[:error]).gsub('|', '\\|')} |"
end
markdown.concat(['', '## Boundaries', ''] + report[:boundaries].map { |item| "- #{item}" })
markdown.concat(['', '## Not verified', ''] + report[:not_verified].map { |item| "- #{item}" })
File.write("#{options[:report_prefix]}.md", markdown.join("\n") + "\n")
puts JSON.generate(passed: report[:passed], tests: report[:tests].length, reports: ["#{options[:report_prefix]}.json", "#{options[:report_prefix]}.md"], failure: report[:failure])
exit(report[:passed] ? 0 : 1)
