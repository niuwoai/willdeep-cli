# 0.154.0-rc5 TUI 轮次收尾投影与队列绑定验收

日期：2026-11-19。结论：轮次产生最终结果后，中间工具调用的已提交编辑 diff 自动折叠，工具行保留且点击可重新展开；本地待发队列的记录绑定按真实占位行校验。Rust 原生 TUI 回归 684 项通过（0 失败，2 项既有手工性能测量忽略），严格 Clippy、格式、版本一致性与差异空白检查通过；真实 PTY 共 78 项检查通过。机器可读记录见 [verification.json](verification.json)。

本记录生成时 rc5 **未提交、未推送、未安装到本机或 q3、未公开发布**。发布 tag 只能指向已合入默认分支的提交，本轮不创建 tag。

> 2026-11-19 更新：rc5 后续已提交、合入 `develop` 并正式发行，安装到本机与 q3、q4-pub、h2.niuwoai.com。见[正式发行](official-release.md)、[本机](local-installation.md)、[q3](q3-installation.md)、[q4](q4-installation.md)、[h2](h2-installation.md)。以下验收记录保留生成时点。

## 版本与候选

| 项目 | 值 |
| --- | --- |
| 版本 | `willdeep 0.154.0-rc5` |
| 原生候选 | `.claude/target/debug/willdeep`（本机 worktree 构建） |
| SHA-256 | `b1cd13cb9b9189cc60819e197044c4531232b3bb447c6326ae56860e800d5e0d` |
| PTY 交叉核对 | 四份报告均报告 rc5，二进制摘要在开始与结束一致 |

## 改动

- 编辑 diff 的自动展开只属于进行中的轮次；轮次进入终态后自动折叠，只留一行工具回执。用户手动展开的差异跨收尾保持，点击仍可重新展开；真实回执、颜色、行号、截断、脱敏与 `/tools` 语义不变。
- 渲染缓存加入轮次运行态，收尾后不复用展开帧。
- 本地待发队列只拥有仍是其未改写占位行（`You: ［待发］ …`）的记录；切换会话清空旧行号。发送失败后记录被改写成 `You: ? …` 时不再用队列正文替换显示或复制，保留 `?` 且整段复制不丢首字符。

## 结果

| 检查 | 结果 | 范围 / 证据 |
| --- | --- | --- |
| TUI 原生测试 | 684 通过，0 失败，2 忽略 | `cargo test --offline -p willdeep --bin willdeep tui:: -- --test-threads=4` |
| 严格 Clippy | 通过 | `cargo clippy --offline -p willdeep --all-targets -- -D warnings` |
| Rust 格式 | 通过 | `cargo fmt --all -- --check` |
| 版本一致性 | 通过 | `ruby scripts/check_release_version.rb 0.154.0-rc5` |
| 差异空白检查 | 通过 | `git diff --check` |
| 轮次收尾投影 | 7 通过 | [JSON](turn-end-projection/turn-end-projection.json) / [Markdown](turn-end-projection/turn-end-projection.md) |
| 工具差异回归 | 25 通过 | [JSON](tool-visuals/tool-visuals.json) / [Markdown](tool-visuals/tool-visuals.md) |
| 送达颜色回归 | 36 通过 | [JSON](status-symbols/status-symbols.json) / [Markdown](status-symbols/status-symbols.md) |
| 队列时间线回归 | 10 通过 | [JSON](queue-timeline/queue-timeline.json) / [Markdown](queue-timeline/queue-timeline.md) |

## 真实终端覆盖

私有 `HOME`、`WILLDEEP_HOME`、工作区与回环合成 Provider；不连接真实模型、不读真实凭据、不写宿主剪贴板。full/compact 各确认：

- 轮次仍在运行时，成功 `create_file` 的路径、增删行数、行号与绿色差异可见。
- 最终回复出现、轮次结束后，差异与文件摘要消失，只剩 `✓ create_file` 工具回执；最终回答与结束分隔线正常。
- 点击保留的工具行后，差异按真实回执重新展开，颜色与行号不变；整个过程没有额外 Provider 请求。

选定画面：

- [full：运行中的差异](turn-end-projection/screens/full/01-running-diff.svg)
- [full：最终结果后仅保留工具回执](turn-end-projection/screens/full/02-after-final-result.svg)
- [full：点击重新展开](turn-end-projection/screens/full/03-reopened-diff.svg)
- [compact：最终结果后仅保留工具回执](turn-end-projection/screens/compact/02-after-final-result.svg)

工具差异、送达颜色与队列时间线使用原有验收脚本重跑，覆盖当前命令与历史命令折叠、颜色与提示、点击/拖选/缩放/原文复制、出队顺序与草稿保留。

## 复核修复

独立只读复核在上一提交 `240c7f51` 上报告三项可验证缺陷，本轮全部修复并补原生回归：

1. 切换会话时旧本地队列行号会指向新会话同下标的用户消息；`load_session` 现在清空 `transcript_row`。
2. 本地队列发送失败后，未确认回执被待发正文替换，`?` 被隐藏且复制丢首字符；显示与复制现在只在记录仍是未改写占位行时使用队列正文。
3. 缓存键未覆盖上述状态下的展开与运行态；缓存加入运行态，记录内容仍按原始来源判定。

## 失败与边界

第一次轮次收尾 PTY 在夹具层失败：脚本按 0 基下标访问 Provider 请求，实际传入 1，两布局均在发出工具前退出。产品代码未改动；修正夹具下标后重跑 7 项通过。失败目录保留在本机临时区，未提交。

- 覆盖 macOS 本机、真实 Runtime、合成 Provider；未验证真实模型回包质量、Windows/Linux 原生终端。
- 轮次收尾 PTY 为英文，送达颜色 PTY 覆盖英文与中文；日文与极窄宽度由原生回归覆盖。
- 未做本机安装、q3 升级、公开发布或版本 tag。

相关文档：[轮次收尾投影决策](../../decisions/2026-11-19-tui-turn-end-projection.md)、[工具视觉决策](../../decisions/2026-11-17-tui-tool-visuals.md)、[CLI TUI 产品说明](../../product/cli-tui.md)。
