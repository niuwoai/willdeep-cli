# 0.152.0-rc4 TUI 状态符号与统计行验收

日期：2026-11-17。结论：本次改动的原生检查及最终优化应用的 56 项真实 PTY 检查通过。此前失败和第一份应用的结果保留，不能代替最终应用证据。

## 最终应用

- 版本：`willdeep 0.152.0-rc4`。
- 冻结副本：`.claude/scratch/tui-rc4-final2/willdeep`。
- SHA256：`a776238e14831a97592c56704e3d56945ddb6500ea48ad0fde1eb99ac7511e44`。
- Web 资源由 `yarn --cwd web build` 构建；最后两处调整仅涉及 Rust，随后再次执行 `cargo build --offline --locked --release -p willdeep`，使用新目录冻结最终应用。
- 三份最终 PTY JSON 的版本、SHA256、逐项通过状态和数量均已与该副本交叉核对。

## 结果与覆盖范围

| 检查 | 结果 | 范围 / 证据 |
|---|---|---|
| TUI 原生测试 | 670 通过，0 失败，2 忽略 | `cargo test --offline --locked -p willdeep --bin willdeep tui:: --no-fail-fast` |
| 严格 Clippy | 通过 | `cargo clippy --offline --locked -p willdeep --bin willdeep --tests -- -D warnings` |
| Rust 格式 | 通过 | `cargo fmt --all --check` |
| 版本一致性 | 通过 | `ruby scripts/check_release_version.rb 0.152.0-rc4` |
| 差异空白检查 | 通过 | `git diff --check` |
| 状态符号与统计点击 | 28 通过 | [JSON](status-symbols-final2/status-symbols.json) / [Markdown](status-symbols-final2/status-symbols.md) |
| 流式阅读、选区与复制 | 18 通过 | [JSON](tui-interactions-final3/integration.json) / [Markdown](tui-interactions-final3/integration.md) |
| 队列时间线 | 10 通过 | [JSON](queue-timeline-final2/queue-timeline.json) / [Markdown](queue-timeline-final2/queue-timeline.md) |

原生测试覆盖中英日、8/40/100 列、各真实状态及异常终态、正文中的同形符号、Unicode/缩进/空格复制、full/compact 点击/拖选/缩放后的完整统计单元格。两项忽略是既有手工流式性能测量，本次未运行。

真实 PTY 启动最终 CLI 和各自的 Runtime：状态符号覆盖 full/compact × 中英，确认 `▶`、`◇`、`✓` 来自实际处理顺序，普通完成活动区及底栏不再出现重复状态短句。鼠标按下、拖选、缩放和返回最新均保留完整统计与未发送草稿；跨过回执符号拖选后，OSC 52 捕获内容精确等于用户原文。流式交互另核对增量到达时的阅读锚点、完整选区、缩放、排队、单次点击返回最新、Provider 请求数及 Runtime 保存结果。队列检查覆盖实际出队顺序、晚到输出、清队列确认和草稿保留。

full 在运行与完成时活动区高度不同，交互夹具现等待第二条回复持久化，再等待完成边界后的完整应用帧，从该帧定位点击目标；不使用固定延时猜测当前坐标，也不通过重复点击掩盖失败。修正前的 [16 项通过后超时记录](tui-interactions-final2/integration.json)保留。

## 实际画面及边界

- [full 中文：选中统计行](status-symbols-final2/full-zh-CN/03-selected.svg)。
- [compact 中文：选中统计行](status-symbols-final2/compact-zh-CN/03-selected.svg)。
- [设计决策](../../decisions/2026-11-17-tui-status-symbols.md)。
- [历史失败与修正](verification-history.md)。
- [机器可读汇总](verification.json)。
- 本机安装和 Runtime 交接另见 [安装记录](local-installation.md)。

所有 PTY 使用私有 HOME、WILLDEEP_HOME 和工作区，只连接回环合成 SSE Provider；不继承真实凭据，不调用真实或付费模型。OSC 52 只由夹具捕获，不转发到宿主剪贴板。完整 ANSI、逐帧 JSON 与中间 SVG 留在本机，仓库只提交总报告和选定画面。

本轮覆盖此次 TUI 显示修正及相关交互，未重跑完整 Rust 工作区、Web 全测试、真实模型或 Windows 原生终端；rc3 的完整工程及 P0/P1 验收仍属于 [rc3 历史组合](../0.152.0-rc3/p0-p1-closeout.md)，不冒充 rc4 新结果。当前仅本机试用，未合并、未公开发布，未创建发布 tag。
