# Rust 免费 Worker 验收入口

`willdeep agents rsi free-review` 只允许确定性 Mock，实际经过原生 Worker 和 `read_file`。不构建网络 Provider，不加载用户凭据，原有 native-trial 和普通 Worker 输出保持原样。结果均标记 `offline_mock_only` / `unverified`，不能作为模型质量或晋升证据。

此命令在全局参数解析前进入专用解析器，只接受这里列出的 Mock 参数。`--api-key`、`--api-base`、`--model`、`--config` 等全局 Provider 参数无论放在命令前后均被拒绝，避免读取全局密钥环境变量或用户配置。

```sh
willdeep agents rsi free-review --offline-mock-only --fixture bug --output-directory /tmp/free-review-bug-new
willdeep agents rsi free-review --offline-mock-only --fixture clean --output-directory /tmp/free-review-clean-new
```

输出目录必须不存在。其中 `raw-report.json` 是真实 Worker 返回的唯一四字段 JSON，`evidence.json` 是供独立宿主评分读取的文件证据；`observation.json` 包含 Mock 请求回执、Worker 事件、实际定义与提示词哈希；`effective-prompt.txt` 用于核对共享输出指令。独立宿主执行评分另行记录，本入口不会执行模型提交的代码。

`worker_context` 完整保留原生 Worker 证据；其中历史字段 `effective_prompt_sha256` 对编译的提示词前缀取哈希。Agent 还会追加运行时预算上下文，因此每个已准入请求的实际系统消息原文与哈希分别记录在 Mock 回执的 `system_prompt` 和 `system_prompt_sha256` 中，可逐轮独立重算；首个实际请求另外保存在 `effective-prompt.txt` 与观察记录 `effective_prompt_sha256` 中。`request_context_projection` 明确标为 `derived: true`，供关联首个已准入请求使用，不能冒充原生 Worker 自行记录的证据；零准入时它为空，不保存请求捕获。

共享格式来自 `bench/agent-rsi/review-output-v2/contract.json`，样例来自同目录 `golden.json`。公共文件来自 `bench/agent-rsi/reproduction-free-v1/fixtures/count-zero-{bug,clean}.js`。基线定义修订固定为 `51b79a923ceceeae80a7cc6e20f2f0dbdfdb136f1bc516b895450ea633273bdd`；格式覆盖只改变本次离线 Worker 提示词，不修改定义。

预算验收示例：

```sh
# 三次真实读取需要四次 Mock 请求：2次上限停止，4次完成。
willdeep agents rsi free-review --offline-mock-only --read-rounds 3 --max-requests 2 --output-directory /tmp/free-review-cap2-new
willdeep agents rsi free-review --offline-mock-only --read-rounds 3 --max-requests 4 --output-directory /tmp/free-review-cap4-new
# 每次已完成Mock回执明确报告50个合成token，49个在准入前拒绝，50个收到首回执后耗尽。
willdeep agents rsi free-review --offline-mock-only --max-tokens 49 --output-directory /tmp/free-review-admission-new
willdeep agents rsi free-review --offline-mock-only --max-tokens 50 --output-directory /tmp/free-review-exhaustion-new
# 响应延迟使真实Worker保持在途，宿主超时或取消会丢弃该Worker future。
willdeep agents rsi free-review --offline-mock-only --mock-delay-ms 5000 --timeout-ms 500 --output-directory /tmp/free-review-timeout-new
willdeep agents rsi free-review --offline-mock-only --mock-delay-ms 5000 --cancel-after-ms 250 --output-directory /tmp/free-review-cancel-new
```

准入拒绝不生成请求回执；在途取消/超时保留 `usage: null`。所有用量均为合成 Mock 用量，网络模型请求数始终为0。预算停止会在正常观察结果内返回 `completed: false` 和明确原因，调用者必须读取观察状态，不能仅靠进程退出码判断任务成功。
