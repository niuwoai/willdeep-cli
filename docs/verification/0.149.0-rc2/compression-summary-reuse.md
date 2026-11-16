# Rust 压缩摘要复用验证

状态：本次三处修复通过本地验证，23 项新增用例全部通过；五组旧行为反验均在预期断言处失败，临时改动已完整恢复。本地提交已另行获授权；推送、合并、打 tag、安装与发布不在本次授权范围内。验证结论受本文“未验证边界”约束。

## 范围与基线

- 工作树：`/Users/rocky/Sites/willdeep-rs/.claude/worktrees/compression-summary-reuse-de99ea`
- 分支：`refs/heads/claude/compression-summary-reuse-de99ea`
- 修复与测试基线：`a202ff87c942a15ec6883a6d4c3ee2c174e6a1e6`，对应验证时的 HEAD 与本地 `refs/heads/develop`；本次本地提交基于该提交。
- 仓库：`niuwoai/willdeep-cli`
- 版本：`0.149.0-rc1` → `0.149.0-rc2`，实际构建二进制输出 `willdeep 0.149.0-rc2`。
- 原调查 HEAD `ce73f70959e9d170795b33d7ee4d29f78a7aa867` 有一个当前 develop 未包含的假等待动画修复。本次未引入该无关提交，当前基线不能表述成包含该调查 HEAD。
- 工程阶段：既有项目迭代，沿用现有 Rust / TUI / Runtime 与会话存储架构。
- 工具链：Rust / Cargo `1.94.1`，macOS 本地执行。
- 所有编辑仅发生在本工作树；未修改 `multica-claude-adapter` 的任务文件。

## 实现

### TUI 本地轮次与通知

`event_loop.rs` 的 `UiMessage::Finished(Ok(...))` 使用 `persist_finished_outcome`：在执行权仍被持有时刷新检查点，随后将 `outcome.messages` 和 `outcome.context_summary` 一起保存，包括返回 `None` 的情况。

`persist_turn_result` 的并发兜底采用现有 `Session::adopt_execution_snapshot`，一起复制消息、摘要、计划、压缩边界、用量与执行检查点，并保留磁盘上的最新外部元数据。普通 TUI 输入仍默认走 Runtime，本问题的入口范围是 `/local` 与通知。

### 手动压缩

保留原 `compress_history_recorded` 无缓存兼容入口，新增唯一的 `compress_history_with_summary_recorded`。TUI `dispatch_compress` 与 headless `execute_noninteractive` 均传入持久摘要。

摘要先以原 durable 历史校验前缀指纹、稳定 system 指纹与完整工具批次边界，再经分页投影映射 `through`；UNKNOWN 回执插入后重新映射覆盖边界。通知去除、HostInstruction 去重与分页变化不应制造缓存失效或错位。

有效摘要投影满足容量目标时直接物化，不发摘要请求；需要继续折叠时仅使用一份已有摘要与未覆盖材料。尾部过长的二次摘要不再重发第一次处理过的原文。真实前缀或稳定规则变更仍使缓存失效。原始用户约束、reasoning replay、UNKNOWN 效果保护、工具批次及容量检查保留。宿主物化后沿用 `replace_with_compressed_messages` 清除旧缓存锚点。

自动与手动入口共用 `stable_system_message(&ProjectRules)`。自动入口保留本轮 `ProjectRules` 的生命周期、刷新和已激活嵌套范围；手动入口从当前根规则开始，不因历史工具调用激活休眠的嵌套规则。实时状态仍按 plan → goal → budget 顺序追加，不参与稳定摘要指纹。

### 持久化 Worker

`subagent/checkpoint.rs` 将持久摘要交给 `run_checkpointed_with_summary`；在执行所有权释放前，基于最新磁盘快照更新返回的消息和摘要，并保留联邦通知。不会以运行前旧会话覆盖运行中已保存的计划、用量、检查点或外部元数据。

真实 verifier 回归走 `run_subagent → run_once → checkpoint::run`，沿用同一 `agent_id`，通过临时目录中的 `test -f ready.txt` 真实失败一次后重试。

## 新增回归

23 项新增用例均已实际通过。不把重复运行或原调查阶段的两轮 51 项测试累加成更多独立用例。

| 范围 | 新增数 | 实际边界与关键断言 |
| --- | ---: | --- |
| 核心手动压缩 | 11 | 真正自动产生摘要后手动零额外请求；二次折叠实际请求原料；增量/无增量；前缀与规则失效；durable/投影映射；UNKNOWN、分页、嵌套规则与受保护内容 |
| TUI Finished 与并发保存 | 6 | 真正 `/local` 解析、dispatch 与通知；接收真实 `UiMessage::Finished` 后调用生产共用保存函数；跨轮复用、摘要更新、并发到达的旧摘要被 `None` 清理、完整执行快照 |
| 手动压缩宿主 | 2 | 真正 TUI `dispatch_compress` 与 headless `execute_noninteractive("/compress")`；自动摘要经过保存及重新加载；请求数不增加、物化清锚点、压缩代数、执行权 |
| Worker checkpoint | 3 | 重新创建 Agent 后恢复摘要；增量原料排除旧前缀；真实计划与并发元数据、用量、执行权及检查点 |
| Worker verifier 重试 | 1 | 真实 verifier 第一次失败、第二次成功；同一 ID 恢复磁盘摘要，摘要请求次数不增加 |

TUI 并发快照夹具对计划、压缩代数及压缩检查点设置不同的非默认值，并在调用兜底前确认与 stale 快照不同，以防默认值相等掩盖漏复制。此处用显式夹具值验证快照合并，不冒充一次真实手动压缩。

## 最终执行结果

Rust 命令统一使用本工作树 `target`，并设置 `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0`。所有模型调用均为本地 mock；未读取真实凭据，未请求真实/付费模型。

| 命令/检查 | 实际结果 |
| --- | --- |
| `cargo test --offline --locked -p willdeep-core --lib` 反验恢复后 | **1049 passed / 0 failed / 6 ignored / 0 filtered out**；共发现 1055 项，其中 1049 项通过、6 项忽略；退出码 0，测试阶段 420.71 秒 |
| `cargo test --offline --locked -p willdeep-core --lib agent::context::tests -- --nocapture` 修正分页夹具后 | 62 passed / 0 failed / 0 ignored / 993 filtered out；属于上行核心库范围，不重复累加 |
| `cargo test --offline --locked -p willdeep --bin willdeep local_summary_persistence -- --nocapture` 反验恢复后 | **6 passed / 0 failed / 0 ignored / 1613 filtered out** |
| `cargo test --offline --locked -p willdeep --bin willdeep materializes_the_real_saved_summary_without_provider_requests -- --nocapture` 反验恢复后 | **2 passed / 0 failed / 0 ignored / 1617 filtered out** |
| `cargo test --offline --locked -p willdeep --test headless_runtime compress -- --nocapture` | **4 passed / 0 failed / 0 ignored / 58 filtered out** |
| `cargo clippy --offline --locked --workspace --all-targets -- -D warnings` 反验恢复后 | 退出码 0，补强测试后的最终复查通过 |
| `cargo fmt --all -- --check` 反验恢复后 | 通过 |
| `LC_ALL=en_US.UTF-8 ruby scripts/check_source_size.rb` | 通过，1503 个受检文件，无超过 3000 行的违规文件 |
| `ruby scripts/check_release_version.rb 0.149.0-rc2` | 通过，Rust、Web、npm、产品概览、架构与 CHANGELOG 版本一致 |
| `git diff --check` | 通过 |
| `yarn --cwd web install --offline --frozen-lockfile && yarn --cwd web build` | 退出码 0；4324 modules transformed，有大 chunk 提醒 |
| `LC_ALL=en_US.UTF-8 ruby scripts/test/npm_willdeep_test.rb` | **7 runs / 38 assertions / 0 failures / 0 errors / 0 skips** |
| `target/debug/willdeep --version` | `willdeep 0.149.0-rc2` |

headless 集成的四项实际为：持久摘要跨运行复用、本地自动压缩约束/工具批次/用量、Web 压缩由 Harness 处理、本地压缩桥保留约束后续跑。均使用临时目录和本地 mock。该集成运行时的生产实现与反验恢复后逐字节相同，恢复后未重复运行。

## 独立审查

收敛工作流完成后，manual、TUI、Worker 三路分别只读审查当前源码及调用链；没有发现可确认的实现缺陷。TUI 审查提出两个测试缺口：未覆盖旧摘要被 `None` 清理，部分执行快照字段使用相同默认值。已补充对应测试和有差异的字段值；补强后的 6 项 TUI 回归实际全部通过。

审查不替代测试运行。审查明确的边界：TUI 测试调用生产共用保存函数，没有驱动整个终端事件循环；手动 TUI 测试接收 `Compressed` 后在测试中执行刷新与保存。Worker 既有取消测试验证消息和所有权，不直接覆盖“新摘要刚生成后取消”。

## 失败与修正记录

1. 初期协作消息恢复了额外写者，造成重复 API/缓存块、递归和重复 move；已停止原工作流及额外写者，改为唯一手动实现者，收敛后编译通过。这些预编译失败没有计入测试通过数。
2. 新测试误用 `Message` 的 `PartialEq`，编译报 E0277；改为完整序列化值比较。
3. 原生核心首轮 **1048 passed / 1 failed / 6 ignored**。唯一失败是分页新夹具：修改 `agent.config.context_window` 不改变 `learned_context_window`。改为调用真实 `tighten_context_window` 后，62 项上下文回归及完整核心库均通过。
4. `None` 补测第一次执行 **5 passed / 1 failed**：夹具误以为短历史会清除传入旧缓存，但 Agent 既有缓存保留策略不会这样返回 `None`。补测改成真实无缓存短轮次结束后，由并发写者放回旧摘要，再验证 Finished 的 `None` 覆盖刷新得到的旧值；不改动既有 Agent 缓存保留语义。最终六项均通过。
5. 初次源码长度检查因 US-ASCII locale 报错；指定 UTF-8 后通过。实现过程中 fmt 曾失败于尚未格式化的新增测试，格式化后检查通过。

## 反验

临时恢复下列旧行为后，五条命令均实际执行用例并以 101 退出。未将编译错误当作反验成功，预期失败不计入正式通过数。

| 临时恢复的旧行为 | 回归实际结果 | 断言证据 |
| --- | --- | --- |
| 手动尾部二次摘要改回发送完整原文 | 0 passed / 1 failed | 第二次请求出现 `RAW_PREFIX_0`，原料排除断言失败 |
| Worker 改回 `run_checkpointed`，不恢复/保存返回摘要 | 0 passed / 1 failed | 真实 verifier 第二次尝试时，磁盘摘要缺失，`the first attempt must durably save its summary before verifier retry` 失败 |
| TUI 并发兜底只替换消息 | 0 passed / 1 failed | 保存的仍是 `SUMMARY_1`，权威执行快照要求 `SUMMARY_2`，消息/摘要一致性断言失败 |
| TUI 与 headless 手动宿主均传 `None` | 0 passed / 2 failed | 两入口均捕获到请求数从 1 增为 2，零额外请求断言失败 |
| Finished 不保存返回摘要 | 1 passed / 5 failed | 跨轮复用与通知保存断言捕获 `None`；并发旧摘要清理断言捕获本应清空的 `Some(old)` |

所有临时生产改动已经恢复；`event_loop.rs`、`dispatch.rs`、`harness.rs`、`agent/context.rs`、`subagent/checkpoint.rs` 的 SHA-256 均与反验前逐一相同，原始指纹见配套 JSON。恢复后重新执行了完整核心库、八项宿主及严格 Clippy，并全部通过。

## 未验证边界

- 不使用真实模型，不衡量真实 Token 费用、用户现场等待时长或实际服务端前缀缓存命中率。
- 未安装或发布，未启动用户已有真实 daemon；未驱动完整 TUI PTY 交互。
- 未运行 Windows/Linux 实机；部分 Worker verifier 测试受现有 Unix 模块条件限制。
- 未运行付费 live-fire、手动范围预检或指定外部账本基准。核心库六个既有 ignored 项为 `checkpoint::tests::execution_lock_probe`、`input_suggestion_livefire::input_suggestion_live_fire`、`livefire::every_verifiable_sample_starts_red`、`livefire::skill_worker_range`、`subagent::file_leases::tests::lease_child`、`subagent::runs::team::ledger::tests::local_ledger_read_only_benchmark`；其中两个是由父用例调用的子进程入口。
- Worker 并发保存用例返回摘要为 `None`，没有直接覆盖“非空返回摘要与并发保存同时发生”；取消用例也没有直接覆盖“新摘要生成后立即取消”。
- 本次运行压缩相关 CLI 宿主和集成用例，没有运行 CLI 的全部单元/集成套件。

## 变更文件

生产逻辑：

- `crates/willdeep-cli/src/tui/event_loop.rs`
- `crates/willdeep-cli/src/tui/dispatch.rs`
- `crates/willdeep-cli/src/harness.rs`
- `crates/willdeep-core/src/agent.rs`
- `crates/willdeep-core/src/agent/context.rs`
- `crates/willdeep-core/src/subagent/checkpoint.rs`

测试与模块声明：

- `crates/willdeep-cli/src/tui/test_suite/local_summary_persistence.rs`（新增）
- `crates/willdeep-cli/src/harness/manual_compression_tests.rs`（新增）
- `crates/willdeep-core/src/agent/context/tests.rs`
- `crates/willdeep-core/src/agent/context/tests/manual_reuse_tests.rs`（新增）
- `crates/willdeep-core/src/subagent/runner.rs`（仅测试模块声明）
- `crates/willdeep-core/src/subagent/checkpoint_summary_tests.rs`（新增）
- `crates/willdeep-core/src/subagent/runner_summary_tests.rs`（新增）

版本与文档：`Cargo.toml`、`Cargo.lock`、`web/package.json`、`npm/willdeep/package.json`、`npm/willdeep/install-manifest.json`、`CHANGELOG.md`、`PRODUCT_OVERVIEW.md`、`docs/ARCHITECTURE.md`，以及本 Markdown 和 JSON 验证报告。提交范围共 23 个文件：16 个既有文件修改与 7 个新增文件；不含 scratch、构建产物或其他任务文件。

## 交接

共享 notes：`/Users/rocky/Library/Application Support/Claude-3p/side-session-notes/local_b599d43b-2f8b-476c-9460-1bbda91dd166/rust-compression-summary-reuse-de99ea.md`。

配套机器可读报告：`docs/verification/0.149.0-rc2/compression-summary-reuse.json`。验证完成后，本地 commit 已另行获授权；推送、合并、打 tag、安装或发布仍未获本任务授权。提交前再次核对现存测试证据与源码，代码未作新增改动，不重复运行耗时测试。
