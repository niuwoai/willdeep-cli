# 0.154.0-rc4 TUI 送达颜色与待发提示验收

日期：2026-11-19。结论：本轮真实 PTY 验收 **36 项全部通过**（full/compact × 中英，每类 9 项），队列时间线 **10 项全部通过**；Rust 原生 TUI 回归与严格 Clippy 同步通过。机器可读记录见 [status-symbols.json](status-symbols/status-symbols.json) 与 [queue-timeline.json](queue-timeline/queue-timeline.json)。

## 版本与候选

| 项目 | 值 |
| --- | --- |
| 版本 | `willdeep 0.154.0-rc4` |
| 原生候选 | `.claude/target/debug/willdeep`（本机 worktree 构建） |
| SHA-256 | `9facff6f78efd10b75ad084cc2015122836d0abe0b068a2627d24425c28714b9` |
| 两端交叉核对 | 两份 PTY JSON 的版本、摘要与实际候选一致 |

本记录生成时 rc4 **未提交、未推送、未安装到本机或 q3、未公开发布**。发布 tag 只能指向已合入默认分支的提交，本轮不创建 tag。

## 结果

| 检查 | 结果 | 范围 / 证据 |
| --- | --- | --- |
| TUI 原生测试 | 679 通过，0 失败，2 忽略 | `cargo test --offline -p willdeep tui:: -- --test-threads=4`；两项忽略为既有手工流式性能测量 |
| 严格 Clippy | 通过 | `cargo clippy --offline -p willdeep --all-targets -- -D warnings` |
| Rust 格式 | 通过 | `cargo fmt --all -- --check` |
| 版本一致性 | 通过 | `ruby scripts/check_release_version.rb 0.154.0-rc4` |
| 差异空白检查 | 通过 | `git diff --check` |
| 送达颜色与提示 | 36 通过 | [JSON](status-symbols/status-symbols.json) / [Markdown](status-symbols/status-symbols.md) |
| 队列时间线 | 10 通过 | [JSON](queue-timeline/queue-timeline.json) / [Markdown](queue-timeline/queue-timeline.md) |

原生测试覆盖中英日、8/40/100 列、十种真实状态组合、同文不同 Turn 身份、重复快照与晚到队列、未确认提交边界、正文中的同形符号（`✓`、`▶` 等必须保持原文）、跨整块复制、附件、折叠控件与窄屏换行。

## 真实终端覆盖

每组使用私有 `HOME`、`WILLDEEP_HOME` 与工作区，启动真实 CLI 与各自私有 Runtime，Provider 为回环合成 SSE，不连接真实模型、不读真实凭据。`SSH_CONNECTION` 强制 OSC 52，复制只由夹具捕获，不写宿主剪贴板。终端 132×36，full/compact × en/zh-CN：

- 已发消息正文为 Cyan（xterm 索引 6），没有三角图标、没有待发提示。
- 入队消息正文为 Yellow（3），下方一行 Gray（7）+ DIM 的本地化提示：中文严格为“Ctrl+Q 管理待发队列”。
- 提示位于正文下方且不触发发送；`Ctrl+Q` 打开真实队列面板并显示该消息，关闭后草稿仍在。
- 轮到该 Turn 时提示撤除，正文恢复 Cyan；未被提前调度（Provider 请求数保持不变）。
- 两轮正常完成后，每条用户消息各有 Green（2）+ BOLD 的 `✓`；不再出现重复的“本轮已完成”长句。
- 鼠标按下/拖选、宽度 90 后恢复、返回最新：统计行与未发送草稿保持完整。
- 跨过回执符号拖选并复制：OSC 52 捕获内容逐字符等于用户原文，不包含提示、状态前缀或角色标记。

选定画面：

- [full 中文：待发黄色正文与淡色提示](status-symbols/full-zh-CN/01-queued.svg)。
- [full 中文：两轮完成后的绿色对勾](status-symbols/full-zh-CN/02-before-click.svg)。
- [compact 中文：待发黄色正文与淡色提示](status-symbols/compact-zh-CN/01-queued.svg)。

队列时间线另在 full/compact 各确认：排队消息不提前开始；出队用户消息先落位在上一轮完整输出之后；多轮顺序与 Runtime 持久化一致；送达后队列回执消失且未发送草稿保留。

## 修正记录

首次运行 27 项通过后在 `full / zh-CN / sent message is cyan without a triangle or queue hint` 失败。原因在验收夹具，不在产品渲染：full 布局的输入框边框本身固定显示“Ctrl+Q 队列”，而夹具截取“聊天区”时只排除了英文标题 `┌Activity`/`┌Prompt`，中文标题 `┌活动`/`┌输入` 未排除，导致固定快捷键被当成消息提示。已改为跳过 chat 框自身的上边框、在下一个任意语言面板边框前停止；产品代码未改动。修正后四组全部重跑，36 项通过。首次失败输出已被重跑覆盖，此处在报告中保留说明。

## 边界

- 覆盖 macOS 本机、真实 Runtime、合成 Provider；未验证真实模型回包质量、Windows/Linux 原生终端渲染。
- 真实 PTY 覆盖英文与中文；日文由 Rust 原生回归覆盖，未做日文 PTY。
- 未验证极窄真实终端（PTY 固定 132/90 列）；窄屏由 8 列原生测试覆盖。
- 终端不支持逐行字号，“小字”以 Gray + DIM 表达，不改变系统字体。
- 未做本机安装、q3 升级、公开发布或版本 tag；上述候选仅为本 worktree 构建。

相关文档：[送达颜色决策](../../decisions/2026-11-19-tui-delivery-colors.md)、[前序状态符号决策](../../decisions/2026-11-17-tui-status-symbols.md)、[CLI TUI 产品说明](../../product/cli-tui.md)。
