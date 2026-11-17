# rc3：Clippy、提问输入与审批结果 Hook 验收

> 日期：2026-11-17。版本：`0.150.0-rc3`。既有 Rust Runtime 的迭代，针对未提交工作区。机器可读结果见[同名 JSON](clippy-approval-hooks.json)。
> 收口状态：本报告所列回归、全工作区严格 Clippy、核心 Windows 目标 Clippy、格式、版本、篇幅及文档检查均通过。覆盖边界和历史失败见正文。本轮未提交、推送、打 tag、创建 PR、安装或发布。

## 1. 实际改动

用户在 [P0 编码工具验收](../0.150.0-rc2/p0-coding-tools.md) 后授权修复剩余三项 Clippy，并推进其他 P0/P1。审批接线契约见[决策记录](../../decisions/2026-11-17-approval-resolved-hooks.md)，整体状态见[差距分析](../../research/2026-11-17-willdeep-cli-gap-analysis.md#71-后续实施状态01500-rc3)。

- **三项 lint**：只被测试使用的排空辅助函数增加 `cfg(test)`；提问文本不再是未使用的 helper，粘贴通过可直接测试的生产路由进入提问；移除绘制闭包无效的 `drop`。
- **提问输入**：可见提问优先接收粘贴、普通按键与鼠标。粘贴 `yes` 不批准后方审批，`y`/Enter 不确认隐藏的 FullAccess 选择器，重叠点击不透传。保留底层草稿、选区、附件和队列文本。
- **多行绘制**：答案与光标复用编辑器的字素和视觉折行；修复满宽软折行紧接显式换行时多出空行的问题，同时保留真实空行与末尾光标行。
- **P1 审批结果接线**：三个实际审批入口和判官最终放行投递 `approval_resolved`；包含会话、工作区、来源和投递时的审批模式，动作与详情脱敏。审批器来源不等于人工来源，事件不等于工具执行成功。
- **Hook 载荷与诊断**：超长载荷按 JSON 转义成本、结构开销和完整 UTF-8 字符预算裁剪，输出仍是 16 KiB 内的合法 JSON，含 `extra.truncated="true"`。观察 Hook 非零退出、启动失败或超时输出结构化 warning，不改变审批结果。

基线为 `ba894a5906982921411e33486fa699f6f356e43e`，分支 `fix/tui-queue-during-compression`。工作区包含上阶段 P0 与其他并行 TUI 修改；本报告只归属上述修复，不能将整个 diff 归给本轮。

## 2. 实际执行记录

下表记录本轮真实运行；筛选命令可能覆盖相邻模块，不以新增测试数或全套运行数解读。核心与 CLI 完整套件的上阶段结果仍保存在 rc2，不冒充本轮重新运行。

| 检查 | 实际命令 | 结果与范围 |
|---|---|---|
| Hook | `cargo test -p willdeep-core --locked --lib hooks::` | 21 通过，0 失败/忽略；含 6 项新增载荷测试。 |
| 审批 | `cargo test -p willdeep-core --locked --lib tools::approval::` | 23 通过，0 失败/忽略；含 12 项新增审批 Hook 测试。 |
| UNKNOWN | `cargo test -p willdeep-core --locked --lib agent::uncertain` | 7 通过，0 失败/忽略。 |
| 委派 | `cargo test -p willdeep-core --locked --lib tools::delegation::tests` | 2 通过，0 失败/忽略。 |
| 既有阻断门禁 | `cargo test -p willdeep-core --locked --lib tools::tests::a_blocking_hook_stops_the_tool_before_it_runs -- --exact` | 1 通过，确认失败的 PreTool 在实际工具动作前拦截。 |
| 既有观察 Hook | `cargo test -p willdeep-core --locked --lib tools::tests::an_observer_hook_sees_the_call_without_blocking_it -- --exact` | 1 通过。 |
| 提问生产路由 | `cargo test -p willdeep --locked --bin willdeep tui::paste` | 6 通过，0 失败/忽略。 |
| 编辑器筛选 | `cargo test -p willdeep --locked --bin willdeep editor::tests` | 11 通过，0 失败/忽略；包含 7 项编辑器测试及 4 项外部编辑器测试。 |
| 输入选区 | `cargo test -p willdeep --locked --bin willdeep tui::prompt_selection::` | 17 通过，0 失败/忽略；含复杂字素、软折行、滚动、弹层隔离与粘贴。 |
| TUI 会话 | `cargo test -p willdeep --locked --bin willdeep tui::test_suite::session_tests` | 84 通过，0 失败/忽略。 |
| Harness | `cargo test -p willdeep --locked --bin willdeep harness::tests` | 30 通过，0 失败/忽略；不等于单独运行了完整 Harness 构造到 Hook stdin 的端到端场景。 |
| 排空 | `cargo test -p willdeep --locked --bin willdeep drain_` | 8 通过，0 失败/忽略。 |
| 宿主严格 lint | `cargo clippy --workspace --all-targets --locked -- -D warnings` | **通过，退出 0**；覆盖本轮最终 Rust 源码。 |
| Windows 目标严格 lint | `cargo clippy -p willdeep-core --all-targets --target x86_64-pc-windows-gnu --locked -- -D warnings` | **通过，退出 0**；交叉编译与 lint，不是 Windows 原生运行。 |
| Rust 格式 | `cargo fmt --all -- --check` | 通过，退出 0。 |
| 版本一致性 | `ruby -E UTF-8:UTF-8 scripts/check_release_version.rb 0.150.0-rc3` | 通过，退出 0；Cargo、内部版本钉、lock、Web、npm、安装清单及产品文档同步。 |
| 差异空白 | `git diff --check` | 通过，退出 0。 |

核心四组测试来自后台批次 `biq18647k`：21/23/7/2 已完成，后续 CLI 编译因补修而主动停止；该批次整体标为中止，不能当作全批通过。既有 Hook 与 Windows 检查来自 `bj8108ypb`，整体退出 0。最终 CLI 六组及宿主 Clippy 来自 `bp1hhofxr`，整体退出 0；输入选区另经修正筛选名的命令实际运行 17 项。

## 3. 场景与证据

| 场景 | 实际证据 | 结论 |
|---|---|---|
| 提问盖住审批 | `visible_question_consumes_yes_without_approving_a_hidden_gate` | 真实粘贴路由与按键入口消费答案；后方审批接收器仍待定，提问结束后单独的审批粘贴才生效。 |
| 提问盖住权限、附件、队列 | `question_preempts_permission_attachment_and_queue_editors` | 四行文本只进答案，不制造附件、不改队列或主草稿。 |
| 隐藏 FullAccess 确认 | `question_keys_keep_a_hidden_full_access_confirmation_pending` | `y`/Enter 进入答案，权限模式不变，后方确认仍待定。 |
| 重叠鼠标按钮 | `question_mouse_does_not_approve_a_button_behind_it` | 使用 TestBackend 实际绘制，再经 `App::handle_mouse` 点击重叠位置；后方审批未批准。 |
| 多行、窄屏与滚动 | `pasted_multiline_answers_remain_visible_at_the_real_cursor_after_scrolling` | En/ZhCn/Ja × 120/30/9 列 × 6 种文本，共 54 种组合；光标前显示最终 `z`，满行换行无额外空行。54 是一个 Rust 测试中的参数组合数。 |
| 满宽后显式换行 | `exact_width_then_newline_does_not_add_an_extra_blank_row` | `abcd\nx` 在宽度 4 为两行；连续真实空行与末尾光标行保留。 |
| 真实审批与磁盘效果 | `execute_create_file_writes_only_when_approved_and_reports_once` | 经 Registry 的真实 `create_file`：允许后内容正确，拒绝后文件不存在，各仅一条审批结果。 |
| 裁决次数与来源 | 三审批入口、判官放行、判官拒绝/不可用/缺失回落测试 | 每次实际最终裁决投递一次；自动规则免审不补造事件。 |
| 失败观察器 | `observer_failure_and_timeout_do_not_change_any_approver_decision` | 非零退出和超时不改变 AllowOnce/AlwaysAllow/Deny；UNKNOWN 仍只接受显式一次性批准。 |
| 载荷预算 | `hooks::payload_tests` 六项 | 覆盖所有公开字符串字段、extra 长键/值、多字节和全部控制字符转义；真实长 Diff 从 stdin 收到且可解析。 |
| 只读与会话 | `read_only_delegation_never_runs_the_host_approval_hook` 与载荷测试；Harness 同变量注入的源码复核 | 只读委派不运行宿主 Hook；直接 Registry 载荷测试核对会话和脱敏，不把源码复核当完整 Harness 动态证明。 |

两次独立静态复核分别检查审批结果接线和提问/鼠标/满行换行补修；指定范围内未确认新增生产缺陷。复核没有运行 Cargo，动态结论仅来自表中命令。

## 4. 失败历史与修复

- 原 rc2 全工作区 Clippy 退出 101，三处为 `wait_until_idle`、`handle_question_text` 未使用与 `drop_non_drop`；历史记录保留，最终 rc3 严格检查通过。
- 第一版路由补修曾触发 `if_same_then_else`，随后重组生产分发；最终通过针对当前源码。
- `bpc7zj7me` 中提问测试 5 通过、1 失败，退出 101；串联的后续命令未运行。`bx13s76ch` 的单项诊断复跑仍失败，退出 101。
- 两次窄屏失败的输出为 `left: " "`、`right: "z"`。诊断坐标为 cursor `(2,28)`、错误 previous `(1,28)`；夹具只计算边框，漏算 `modal_block` 的水平 padding。修正为 `modal_block(...).inner(rect)` 获取真实内容区，保留“光标前是 z”的断言。最终六项及全部 54 种组合通过，没有放宽产品行为断言。
- 命令筛选 `tui::prompt_selection_tests` 实际匹配 0 项，退出 0 不计作覆盖；改为 `tui::prompt_selection::` 后实际 17 项通过。
- 外部编辑器测试出现 `willdeep-no-such-editor-xyz: ... command not found`，来自合成缺失编辑器场景，该组 11 项最终通过。故意失败/超时 Hook 的 warning 同属预期反例；测试通过不表示 Hook 子进程退出成功。

## 5. 限制与未交付项

- 本轮没有重新运行核心/CLI 全套、真实 TypeScript LSP、网页夹具或付费模型评测；相关历史覆盖只引用 rc2。当前工具链未提供 `rust-analyzer`，未安装，真实 Rust LSP 尚无证据。
- Windows 仅核心目标交叉 Clippy；没有 Windows 原生运行、完整真实服务 diagnostics 发布、大型项目或所有平台组合的证明。
- Hook 没有持久队列、恰好一次或取消/崩溃补发保证。审批结果不是执行回执；`AlwaysAllow` 结果不证明授权持久化成功。
- 静态复核发现既有长期授权逻辑先插入内存规则、后持久化；持久化失败时规则可能仍留在该 Registry 内存。本轮未改该旧路径，也未执行失败后的二次调用回归，不能宣称此问题已修复。
- 16 KiB 限制约束输出帧。实现先完整序列化判断是否超限，因此不限制首次序列化的中间内存；Hook 失败日志也未另做独立 stderr 捕获断言或强制 shell 启动失败测试。
- P1 本轮完成的是已有审批事件的接线修复。语义重命名/AST、正式受控浏览器、独立 BYOK 搜索、项目级全部辅助模型调用策略和 Goal criterion 的结构化执行证据仍未交付。
- P0 的文件级原子安装不等于跨文件事务或外部进程 CAS；回执取消与 UNKNOWN 边界仍遵循 rc2，不因新增 Hook 而扩大保证。

## 6. 最终文档门禁

| 检查 | 实际方式 | 最终结果 |
|---|---|---|
| 源码篇幅 | `ruby -E UTF-8:UTF-8 scripts/check_source_size.rb --output target/rc3-source-size` | 退出 0；1576 个受检文件，0 个超过 3000 行。超过 2000 行的文件仍在职责审查清单中；本轮将生产粘贴路由独立成模块，提问鼠标逻辑移出 `app_state.rs`，没有为 lint 无关重构其他模块。 |
| 文档与 JSON | `ruby -E UTF-8:UTF-8 target/rc3-document-check.rb` | 退出 0；10 份文档的 277 条本地文件链接均存在；检查文件目标，不验证 Markdown 标题锚点。两份验收 JSON 可解析，rc3 的 17 条命令和测试数与 Markdown 一致，渲染组合数核对为 54。 |
| 产品概览 | 同一 Ruby 核查 | 16553 个 Unicode 字符，小于 20000 上限。 |
| 差异空白 | `git diff --check` | 最终文档编辑后再次通过，退出 0。 |

文档核查脚本位于本地 `target/` 工作目录，用于本轮只读检查；它不进入产品运行链。rc2 报告仅增加后续 rc3 链接，保留当时的失败结果；差距分析保留原始调研基线，另列后续实施状态。
