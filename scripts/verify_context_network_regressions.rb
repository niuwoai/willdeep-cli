#!/usr/bin/env ruby
# frozen_string_literal: true

require 'json'
require 'open3'
require 'fileutils'
require 'digest'

root = File.expand_path('..', __dir__)
binary = File.expand_path(ARGV.fetch(0) { abort '请传入本次编译的 willdeep-core 测试二进制路径' })
raise 'expected compiled test binary missing' unless File.executable?(binary)
names = %w[
  mcp::http::tests::an_expired_session_is_reinitialized_once
  mcp::http::tests::speaks_streamable_http_with_session_and_protocol_headers
  mcp::http::tests::unauthorized_carries_the_resource_metadata_hint
  mcp::oauth::tests::logs_in_against_a_fake_authorization_server
  provider::anthropic::streaming::tests::interrupted_anthropic_stream_keeps_partial_text_and_usage_without_retry
  provider::anthropic::streaming::tests::real_anthropic_stream_delivers_text_and_cumulative_usage
  provider::chat_completions::streaming::tests::interrupted_chat_preserves_text_without_replaying_or_exposing_tools
  provider::chat_completions::streaming::tests::real_chat_provider_requests_stream_and_emits_deltas_and_usage
  provider::chat_completions::streaming::tests::replayed_history_carries_reasoning_and_repairs_broken_tool_arguments
  provider::common::tests::cancelling_retry_wait_never_sends_another_request
  provider::common::tests::client_errors_fail_on_the_first_try
  provider::common::tests::dropped_connection_is_retried_until_it_lands
  provider::common::tests::error_body_is_bounded_before_the_server_finishes_sending
  provider::common::tests::excessive_retry_after_returns_without_sending_early
  provider::common::tests::expired_request_deadline_never_sends_a_request
  provider::common::tests::rate_limit_waits_for_retry_after_before_sending_again
  provider::common::tests::requests_go_through_a_socks5_proxy
  provider::common::tests::retries_give_up_after_the_attempt_budget
  provider::common::tests::server_errors_are_retried
  provider::responses::streaming::tests::real_responses_request_delivers_text_and_authoritative_usage
  provider::responses::streaming::tests::responses_eof_keeps_partial_text_and_does_not_retry
  provider::sse::tests::eof_after_a_valid_event_is_still_incomplete
  provider::sse::tests::network_events_arrive_before_eof_and_completion_closes_the_reader
].freeze
directory = File.join(root, 'docs/verification/0.103.0-rc6')
FileUtils.mkdir_p(directory)
results = names.map do |name|
  output, status = Open3.capture2e(binary, '--exact', name, '--quiet')
  passed = status.success? && output.include?('1 passed; 0 failed')
  puts JSON.generate(test: name, passed: passed)
  { name: name, exit_code: status.exitstatus, passed: passed, output: output }
end
report = { schema: 'willdeep.context-recovery.network-regressions.v1',
           initial_full_suite: { passed: 749, failed: 23, ignored: 6,
                                 failure_reason: 'loopback bind denied by sandbox' },
           binary_sha256: Digest::SHA256.file(binary).hexdigest,
           rerun_count: results.length, passed: results.all? { |item| item[:passed] },
           real_model_requests: 0, results: results }
File.write(File.join(directory, 'network-regressions.json'), JSON.pretty_generate(report) + "\n")
File.write(File.join(directory, 'network-regressions.md'),
           "# 网络夹具回归补验\n\n原全量测试：749 通过、23 项本机监听受沙箱拒绝、6 项既有忽略。\n\n" +
           "对原 23 项逐项原样补验：#{results.count { |item| item[:passed] }}/23 通过。全部为本机假服务器，真实模型请求 0。\n\n" +
           results.map { |item| "- #{item[:passed] ? '通过' : '失败'}：`#{item[:name]}`" }.join("\n") + "\n")
exit(report[:passed] ? 0 : 1)
