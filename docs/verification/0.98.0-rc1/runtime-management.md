# Runtime 管理实际运行验收

版本：0.98.0-rc1。隔离 Provider，未调用真实付费模型。

- task_turn_budget_overrides_global：passed；{"check":"task_turn_budget_overrides_global","status":"passed","actual_status":"partial","model_calls":1,"snapshot_max_turns":1,"run_id":"44a73235-34b7-5ebd-addb-442f9c751bac"}
- task_token_budget_overrides_global：passed；{"check":"task_token_budget_overrides_global","status":"passed","actual_status":"failed","model_calls":1,"reported_tokens":1103,"run_id":"3f1fc1b1-1c63-5f46-9f17-6cf88fd65f08"}
- wall_clock_timeout_no_replay：passed；{"check":"wall_clock_timeout_no_replay","status":"passed","actual_status":"failed","elapsed_seconds":1.28,"model_calls":1,"reason":"scheduled run exceeded its configured wall-clock budget","run_id":"f775ee73-fb7a-5bdd-aa27-6c5d27923dc5"}
- completion_result_and_usage：passed；{"check":"completion_result_and_usage","status":"passed","usage":{"input_tokens":20,"model_calls":null,"output_tokens":3,"total_tokens":23},"verification":null,"run_id":"88ef726a-4510-5ee0-a3da-96d92b18d6a8"}
- archive_delete_retains_history：passed；{"check":"archive_delete_retains_history","status":"passed"}
- completed_chat_receipt_retry_is_idempotent：passed；{"check":"completed_chat_receipt_retry_is_idempotent","status":"passed","model_calls":1,"turn_id":"ef515bf7-8841-46ca-a487-22894746db4e"}
