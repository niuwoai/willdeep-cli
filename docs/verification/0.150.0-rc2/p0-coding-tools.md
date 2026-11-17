# P0 编码工具：分层验收记录

> 日期：2026-11-17。本轮是既有 Rust Runtime 的迭代；功能在 `0.150.0-rc1` 引入，共享工作区当前整合版本为 `0.150.0-rc2`。记录针对未提交工作区，不代表合并、安装或发布。机器可读结果见[同名 JSON](p0-coding-tools.json)。
> 收口状态：P0 实现及所列本地回归完成；核心、CLI、文件协议、LSP、固定任务与两条真实网页交互流程通过。全工作区 Clippy 仍因 P0 范围外的三处 CLI lint 失败；Rust 真实语言服务、Windows 原生运行及真实模型评测未执行。
> 后续：三处 lint 已在 rc3 修复并通过全工作区严格检查，另补审批结果 Hook 与提问输入回归，见[rc3 验收](../0.150.0-rc3/clippy-approval-hooks.md)。本页保留 rc2 当时的失败与覆盖边界。

## 1. 实施范围与保证

实施来源为 [Pi / OMP 差距分析的 P0](../../research/2026-11-17-willdeep-cli-gap-analysis.md#5-建议的补齐顺序)，行为契约见[P0 决策](../../decisions/2026-11-17-p0-coding-tools.md)。

- **版本化编辑**：`read_file` 对整文件计算 SHA-256，版本行与正文分页预算分开；`edit_file` 使用显式版本或当前 Registry 的实际读取基线，缺基线或版本过期时拒绝。成功登记新版本。读取证据消费者同步校验真实工具输出，不接受旧格式或只有元数据的回执。
- **多文件创建/替换**：`file_changes.preview` 只生成绑定路径、原版本、新内容和差异的一次性 ID；`apply` 复用审批、权限与 Worker 写集，全量预检后逐文件安装，失败尝试恢复，并报告冲突、恢复失败、临时文件清理失败与实际遗留路径。只支持 UTF-8 文本，不创建父目录，不支持删除/重命名。
- **LSP 查询**：`status`、`diagnostics`、`definition`、`references`、`symbols`；支持 Rust 与 TypeScript/TSX，定位为 0-based UTF-16。固定本地服务，沿用命令审批和有效围栏，有界传输、取消、进程树与快照管理；不自动安装、不开放语义重命名或服务端 applyEdit。
- **回归基线**：沿用原 ModelEval 任务、独立 verifier 与报告引擎，新增三个编码任务。总任务数由 20 增至 23，validation 由 8 增至 11；原生 Runtime 场景另外映射到实际执行的 Rust 回归。

本进程的创建、编辑、多文件应用共用写入锁；审批等待不持锁。文件级原子安装不等于跨文件事务，也不构成对任意外部进程的 CAS。同步提交与恢复不能被任务取消截断，但之后的 PostTool hook、结果交付或进程退出仍可能导致回执缺失；不能据此推断未写入。Agent 对同工具名与规范化 JSON 参数的未知副作用重试要求显式一次性审批，直接调用 Registry 没有这一历史保护。

## 2. 实际执行记录

以下结果不能相加当作独立测试总数，定向测试与完整套件存在重叠。

| 层级 | 实际命令 | 结果及覆盖边界 |
|---|---|---|
| 核心完整回归 | `cargo test -p willdeep-core --locked --no-fail-fast` | 1108 项单元测试通过、6 项忽略；另 5 项 Provider 契约测试通过。默认套件内的 2 项真实 LSP 测试为忽略，另有显式运行记录。执行耗时约 421 秒。 |
| 最新文件协议边界 | `cargo test -p willdeep-core --locked --lib tools::file_changes` | 34 通过、0 失败、0 忽略；包含独立复核新增的符号链接、非 UTF-8 序列化、二进制与 PostTool 取消场景。 |
| 最新 LSP 离线协议 | `cargo test -p willdeep-core --locked --lib tools::lsp` | 24 通过、0 失败、0 忽略；含固定配置、UTF-16、诊断、路径过滤、overlay、帧上限、主动请求、取消和 Unix 进程树释放。 |
| CLI 全套 | `cargo test -p willdeep --locked --no-fail-fast` | 1648 项单元测试通过、5 项忽略；后台任务 18 通过、1 个子进程探针忽略（探针在父测试中另行执行并通过）；无头 Runtime 62 通过；版本集成 1 通过。原生读取证明与多文件 Diff 归属用例均通过。 |
| Runtime SDK 与协议 | `cargo test -p willdeep-runtime-client -p willdeep-runtime-protocol --locked` | 客户端 7 项单元测试及 1 项 doctest、协议 57 项单元测试通过。 |
| 真实 TypeScript 服务 | `cargo test -p willdeep-core --locked --test lsp_tools -- --ignored --nocapture` | 2 通过；有 tsconfig 与 inferred project 两种项目，均经公开工具入口执行跨文件定义、引用、符号和其他文件 overlay 刷新。 |
| ModelEval 原框架 | `ruby -E UTF-8:UTF-8 scripts/test/model_eval_test.rb` | 32 runs、748 assertions，0 failures/errors/skips。 |
| 新任务红绿与反作弊 | `ruby -E UTF-8:UTF-8 scripts/test/p0_eval_tasks_test.rb --report-dir target/p0-eval-contracts` | 19 runs、116 assertions，0 failures/errors/skips；归档为 29 条契约断言，全部符合预期。 |
| 任务完整性 | `ruby -E UTF-8:UTF-8 scripts/model_eval.rb --check-tasks` | 23 个任务，0 个不合格。 |
| Web 配置桥离线回归 | `node --test scripts/test/config_web_bridge_test.mjs` | 6 通过；只覆盖桥接数据和错误处理，不是浏览器页面交互。 |
| 核心宿主 Clippy | `cargo clippy -p willdeep-core --all-targets --locked -- -D warnings` | 通过。 |
| 核心 Windows Clippy | `cargo clippy -p willdeep-core --all-targets --target x86_64-pc-windows-gnu --locked -- -D warnings` | 通过；为交叉编译与 lint，不是 Windows 原生运行。 |
| 核心 Windows 编译 | `cargo check -p willdeep-core --all-targets --target x86_64-pc-windows-gnu --locked` | 通过；测试代码中的两处 Windows 条件编译告警已修正，并由更严格的目标 Clippy 通过复核。 |
| 全工作区 Clippy | `cargo clippy --workspace --all-targets --locked -- -D warnings` | **失败，退出 101**；仍为 P0 范围外的三处 CLI lint：`task_manager.rs:879`、`app_state.rs:1855` 的 dead_code 与 `human_gate_ui.rs:130` 的 drop_non_drop。核心部分通过，不能将全工作区门禁说成通过。 |
| 格式 | `cargo fmt --all -- --check` | 通过。 |
| 版本 | `ruby -E UTF-8:UTF-8 scripts/check_release_version.rb 0.150.0-rc2` | Cargo、内部版本钉、lock、Web、npm、安装清单、产品、架构与 CHANGELOG 一致。 |
| 源码篇幅 | `ruby -E UTF-8:UTF-8 scripts/check_source_size.rb` | 1571 个受检文件，无文件超过 3000 行；超过 2000 行的责任审查清单由脚本单独输出。 |
| 差异空白 | `git diff --check` | 通过。 |

核心完整套件先于独立复核的最后一组文件协议修补执行；最新修补由后续完整的 34 项文件协议定向回归覆盖。LSP 的最后两处改动只涉及测试的 Unix 条件导入和可变变量范围，随后 24 项 LSP 测试及宿主/Windows Clippy 均通过；没有把较早完整套件冒充为对最后每个字节的重新运行。

## 3. 核心场景到实际证据的映射

| 场景 | 独立证据 | 本轮结论 |
|---|---|---|
| 陈旧/未读编辑与重复片段 | `file_changes/tests.rs` 的未读拒绝、显式陈旧版本、读取基线隔离、重复片段与行号前缀测试 | 由 34 项文件协议套件覆盖；检查磁盘最终内容，不能仅凭成功字符串判断。 |
| 审批、权限、并发与部分失败 | `file_changes/contract_tests.rs`、`review_tests.rs`；跨 Registry 协调、Strict 切档、只读上限、白名单、恢复冲突及清理失败 | 全量预检失败无写入；恢复不得覆盖外部新内容；遗留写集报告绑定的真实相对路径。 |
| Rust 跨文件引用 | `p0-rust-cross-file-references`：pricing、invoice、checkout 联动，包含别名及无关同名项 | 原夹具红、参考答案绿；漏任一必改文件、硬编码、广泛替换或改测试均被拒绝。不等于 Rust 真实 LSP 已验证。 |
| TypeScript 引用与字符串键 | `p0-typescript-symbol-key-references`：formatter、view、目标字符串键与无关普通字符串 | 红绿与反作弊符合预期；Node 类型擦除加固定形态/行为断言，不等于完整 TS 类型检查或已实现语义重命名。 |
| 受保护配置与多文件批处理 | `p0-protected-multifile-config`：batching、manifest、受保护配置和测试 | 漏改、改保护文件、硬编码、配置绕行与额外文件不能通过。 |
| 长上下文与压缩后约束 | `automatic_compression_preserves_constraints_and_tool_protocol`、`compression_preserves_unknown_effects_and_their_replay_guard`、`uncompressible_user_input_never_emits_false_success` | 既有原生测试在本轮核心完整套件实际通过；不是新增固定模型任务。 |
| 多任务恢复与未知副作用 | `foreground_recovery_tools_resume_once_then_return_cached_verified_report`、`recovery_inventory_beyond_4096_paginates_within_parent_and_marks_reports`、`cancelled_after_real_write_before_receipt_cannot_automatically_replay` | 本轮核心完整回归通过；批次与单文件真实回执进入 WorkerFacts；缺回执保持 UNKNOWN。 |
| 错误完成与保护测试 | `test_only_change_cannot_finish_an_implementation_contract`、`contract_checks_current_run_changes_and_protects_existing_tests`、`patch_contract_baseline_survives_disk_recovery_and_cannot_be_weakened` | 本轮核心完整回归通过；不能由此宣称 Goal 的所有文字 evidence 已接入结构化执行证明，该能力仍属原调研 P1。 |
| 网页交互 | 六项配置桥离线回归；通过 Browser pane 操作生产 React 页面与 `web_runtime_retry_fixture.mjs` 的两条停止流程 | 已提交后等待重试时停止、submitted 前停止均通过：仅向正确 Turn 发一次停止请求、关闭 SSE、移除等待与停止按钮、恢复发送。后端为合成夹具；原生浏览器工具仍属 P1。 |
| 真实 TypeScript LSP | `tests/lsp_tools.rs` 的两个显式 smoke | 两种项目跨文件首次查询与 overlay 刷新通过；未验证真实服务器的诊断发布及所有编辑器/大型项目组合。 |
| Runtime 归属与真实读取证明 | CLI 的 `runtime_sink_attributes_child_agent_file_changes_without_chat_metadata`、`native_worker_uses_only_broker_and_real_read_file_with_receipts_and_raw_output`、`read_proof_requires_exact_fixture_result_in_a_delivered_request` | 三项均在 CLI 完整回归中通过；真实读取证明仍要求正确路径、实际工具结果哈希及已送达请求。 |

### 网页交互的独立执行方式

使用现有 `scripts/web_runtime_retry_fixture.mjs` 提供生产 React 页面和本地合成 SSE 后端；Node/Vite 均为已有依赖。通过当前 worktree 的本地预览配置，以清空继承环境的独立 HOME 启动夹具，端口为 19849，不连接 9847。预览工具实际读取了本 worktree 的配置并成功启动；旧环境中“只能读取主仓配置”的历史限制本次没有复现。

没有安装 Playwright，也没有执行依赖 Playwright 的 `scripts/web_chat_stop_test.cjs`。以该脚本的两个场景为参考，改用应用 Browser pane 的 `preview_fill` / `preview_click` 驱动真实输入和按钮，`preview_eval` 只读取状态并执行断言，没有修改页面实现或伪造组件状态：

| 场景 | 操作与中间断言 | 最终断言 |
|---|---|---|
| 已提交、等待重试 | 设置合成模式 waiting；在真实输入框填写并发送；观察 `qa-turn-1` 已 submitted、等待提示可见、停止按钮可用；点击停止 | 对 `qa-turn-1` 的停止 POST 恰好 1 次；服务端 stopped/closed 为 true；等待和停止按钮消失、发送恢复。 |
| submitted 前停止 | 切换合成模式 pending 并重新加载；填写并发送；观察 `qa-turn-2` 尚未 submitted；点击停止后停止 POST 仍为 0、流保持等待；仅通过夹具端点释放 submitted 事件 | 对 `qa-turn-2` 的停止 POST 恰好 1 次；SSE 关闭；等待与停止按钮消失、发送恢复。 |

`/health` 返回版本 `0.150.0-rc2`；最终状态记录两个 Turn 分别仅有一次停止请求，第二个已 closed/stopped。页面资源 origin 全部为 `http://localhost:19849`。完整断言摘要保存在同名 JSON；验收结束已停止夹具。

夹具没有实现 setup、identity、default-model 等元数据端点，因此可见配置警告及相应 404；这次没有宣称控制台全绿或完整后端验收。两个停止交互通过也不等于网页上的编辑/LSP 全链路已单独实测。浏览器原生工具产品化仍属 P1。

## 4. 固定任务报告口径

原始离线归档位于 `target/p0-eval-contracts/reports/2026-11-16/offline-fixture-contracts-no-model.180829Z.{json,md}`。日期目录使用归档时的 UTC 日期；本地验收日期是 2026-11-17。同名最终 JSON 保存任务指纹和 29 条断言的预期/观察值摘要，原始归档属于可重建的构建产物。

29 条记录全部 `passed`，含 3 条 `solution-green`；其余记录期望候选被拒绝，观察也均为拒绝。保护文件被篡改的反例中 `protected_intact=false` 是注入的攻击条件，断言通过表示成功识别并拒绝它。

这里没有调用模型，`contains_model_runs=false`。29 是红绿与反作弊断言数，不是 29 道题，也不是模型正确率。模型 Token、费用和模型成功率不适用，不能按零成本模型运行记账。报告与 history 放在独立目录，禁止写入真实模型历史目录及其符号链接别名。

## 5. 失败、修复与平台边界

- 核心完整套件初轮两项失败：自定义 Agent 的测试移除读取权限后又切换只读角色，以及 Worker fixture 在版本化编辑后仍直接无版本编辑。修正测试设置后，核心完整套件得到 1108+5 全通过结果，没有放宽产品权限。
- 真实 TypeScript 首次定义查询曾只返回本地 import alias。有无 tsconfig 都复现；根因是 `typescript-language-server` 的默认语法服务路由。固定 `tsserver.useSyntaxServer=never` 后两种项目均通过，未用 sleep、重试或降低跨文件断言掩盖。
- CLI 原生 Worker 机制验证曾失败：读取证明重建的仍是旧版无版本头输出。现在同步版本头、正文预算与续读格式，并保留伪造路径、未送达回执、旧格式及只有版本元数据的拒绝反例。相关 read-proof 与 native Worker 用例均已在最终 CLI 完整回归中通过。
- 独立复核修补了目录符号链接下创建的写集复核、别名改指后的遗留写集归属、非 UTF-8 绝对路径序列化 panic，以及二进制旧内容有损 diff。新增故障注入直接检查最终磁盘与回执；34 项已通过。
- macOS APFS 不接受非法文件名字节，早期非 UTF-8 工作区根目录 fixture 在建目录时失败。现将无磁盘序列化测试保留为 Unix 通用，真实非法字节根目录 I/O 场景仅在非 macOS Unix 编译；本机未执行该 Linux 场景。
- CLI 全套两次在并行 TUI 代码修改期间编译失败，分别为 `open_permission_picker` 新参数及剪贴板 helper 参数/可见性不一致；当前代码已核对修正并重验，不将中间失败运行计作测试通过。
- 最早 workspace Clippy 同时发现本轮两处 lint 与 CLI 三处 lint。本轮的锁跨 await、可折叠条件已修复；核心 host/Windows `-D warnings` 已通过。最终全工作区检查仍因 CLI 的 `task_manager::wait_until_idle`、`App::handle_question_text` 死代码与 `human_gate_ui` 的 `drop_non_drop` 退出 101；三项均保留为 P0 范围外阻塞，没有用全局 allow 或修改这些实现隐藏。

真实服务环境为 `typescript-language-server 5.3.0`、Node `v24.8.0`、Rust `1.94.1`。Rust 语言服务虽然可解析到 rustup 代理，但代理报告当前 toolchain 不含可用 `rust-analyzer`；没有自动安装。因此 **Rust 真实语言服务未验证**，不能把依赖路径可解析等同于成功启动。

Windows 已验证核心 all-targets 交叉编译和严格 Clippy，未在 Windows 原生环境运行测试。语言服务进程不是天然只读，现有写入围栏不提供宿主读取隔离；TypeScript 首版拒绝配置继承、插件、project references 和符号链接等限制见决策文档。未收到、无版本、陈旧、过滤或截断的诊断不能宣称当前代码零错误。

## 6. 本轮未执行的事项

没有运行真实模型评测或 Pi/OMP 对照实验，没有自动安装语言服务，没有替换本机 CLI/Daemon，没有 commit、push、tag、PR 或发布。其他并行会话的 TUI 改动与报告保留在共享工作区，其独立 PTY 结果不计入本报告。
