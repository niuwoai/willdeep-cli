# 网络夹具回归补验

原全量测试：749 通过、23 项本机监听受沙箱拒绝、6 项既有忽略。

对原 23 项逐项原样补验：23/23 通过。全部为本机假服务器，真实模型请求 0。

- 通过：`mcp::http::tests::an_expired_session_is_reinitialized_once`
- 通过：`mcp::http::tests::speaks_streamable_http_with_session_and_protocol_headers`
- 通过：`mcp::http::tests::unauthorized_carries_the_resource_metadata_hint`
- 通过：`mcp::oauth::tests::logs_in_against_a_fake_authorization_server`
- 通过：`provider::anthropic::streaming::tests::interrupted_anthropic_stream_keeps_partial_text_and_usage_without_retry`
- 通过：`provider::anthropic::streaming::tests::real_anthropic_stream_delivers_text_and_cumulative_usage`
- 通过：`provider::chat_completions::streaming::tests::interrupted_chat_preserves_text_without_replaying_or_exposing_tools`
- 通过：`provider::chat_completions::streaming::tests::real_chat_provider_requests_stream_and_emits_deltas_and_usage`
- 通过：`provider::chat_completions::streaming::tests::replayed_history_carries_reasoning_and_repairs_broken_tool_arguments`
- 通过：`provider::common::tests::cancelling_retry_wait_never_sends_another_request`
- 通过：`provider::common::tests::client_errors_fail_on_the_first_try`
- 通过：`provider::common::tests::dropped_connection_is_retried_until_it_lands`
- 通过：`provider::common::tests::error_body_is_bounded_before_the_server_finishes_sending`
- 通过：`provider::common::tests::excessive_retry_after_returns_without_sending_early`
- 通过：`provider::common::tests::expired_request_deadline_never_sends_a_request`
- 通过：`provider::common::tests::rate_limit_waits_for_retry_after_before_sending_again`
- 通过：`provider::common::tests::requests_go_through_a_socks5_proxy`
- 通过：`provider::common::tests::retries_give_up_after_the_attempt_budget`
- 通过：`provider::common::tests::server_errors_are_retried`
- 通过：`provider::responses::streaming::tests::real_responses_request_delivers_text_and_authoritative_usage`
- 通过：`provider::responses::streaming::tests::responses_eof_keeps_partial_text_and_does_not_retry`
- 通过：`provider::sse::tests::eof_after_a_valid_event_is_still_incomplete`
- 通过：`provider::sse::tests::network_events_arrive_before_eof_and_completion_closes_the_reader`
