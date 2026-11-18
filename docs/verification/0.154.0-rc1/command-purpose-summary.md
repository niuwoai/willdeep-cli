# 长命令用途说明验收

日期：2026-11-18。版本：0.154.0-rc1。结果：本轮列明的本地检查通过，尚未安装、提交、推送或发布。

## 交互结果

`run_command` 优先显示当前用户语言的简短用途，例如“补齐文章缺失的标题和摘要翻译”。复用既有 `label`，不新增模型请求。无有效说明的旧调用继续显示命令摘要。

长脚本默认折叠，点击工具行可查看完整脱敏命令，再次点击收起。最新活动短命令在最多 120 字符、3 行时保留自动展开；手动展开优先。活动命令的用途行保持可见，不并入历史工具汇总。用途说明不参与执行、审批或成功判断，状态仍来自真实回执。

本次真实终端捕获的第二条活动命令：

```text
· ⋯ 1 tool calls folded · /tools to expand
· + Thought: 0.1s
· … run_command · 核对后续命令的运行状态
```

## 验证结果

| 范围 | 结果 | 说明 |
| --- | --- | --- |
| Rust 核心与工具契约 | 35 项通过 | 有效 label、旧调用、中文长度、控制符、凭据与路径、历史投影及原始参数不变 |
| CLI 展示、历史和视口 | 88 项通过 | 包含 15 项工具视觉、44 项 narration、17 项 viewport、11 项历史折叠、1 项摘要兼容 |
| Web 消费链路 | 11 项通过 | RunCard 9 项与 replayTools 2 项 |
| full/compact 真实 Runtime + PTY | 29 项通过 | 每布局连续两条长命令、默认折叠、点击展开与收起、真实命令回执，并保留编辑差异和历史邻接检查 |
| 真实 Web HTTP 投影 | 4 项通过 | HTTP 200、版本 header、中文用途、中文紧邻路径隐藏、原始脚本与 arguments 不进入消息投影 |
| 构建与静态检查 | 通过 | Web tsc/Vite、Rust debug 构建、cargo fmt、严格 Clippy、版本一致性和 diff 检查 |

Web 通过既有历史会话入口加载，展开工具汇总后实际 DOM 包含“补齐文章缺失的标题和摘要翻译”与“检查目录：…”。最终页面未记录控制台错误或失败网络请求。普通点击未观察到展开状态后，以 DOM click 事件触发现有按钮处理器，确认 `aria-expanded=true` 和上述两条用途说明，并保存真实页面截图；未注入产品界面。

最终 HTTP 工具投影：

```json
[
  {"name":"run_command","detail":"补齐文章缺失的标题和摘要翻译","completed":true},
  {"name":"run_command","detail":"检查目录：…","completed":true}
]
```

## 源码与构建绑定

- 源码分支：`feat/command-purpose-summary-20261118`。
- 源码工作区：`/Users/rocky/Sites/willdeep-rs/.claude/worktrees/command-purpose-summary-20261118`。
- 基线提交：`8372c94203ea5514d289a857e3f953d8ab360c83`；功能修改仍未提交。
- 冻结应用：[final/willdeep](../../../target/command-purpose-0.154.0-rc1/final/willdeep)。本轮使用 debug 构建，未声称 release 安装验收。
- 应用 SHA256：`8515a9c558e72e7e80cc706b948cea34c3b5be66611e29ca965535954b84b50c`。PTY 验收前后完全一致，HTTP 验收使用同一应用。
- 代码及构建元数据差异 SHA256：`d5ee2709d37fa6173a87698fee3899b187b95c4d164ee0bf1884d5a4bc5e653d`，输入为 `git diff --binary -- crates Cargo.toml Cargo.lock web/package.json npm/willdeep scripts/verify_tui_tool_visuals.rb`，不包含后续验收文档。

## 失败历史与修正

首轮真实 PTY 在 full 和 compact 的第二条长命令均超时，整轮失败。原因为历史聚合仅保留展开行，默认收起的活动长命令被一并隐藏。修复后新增活动摘要回归，并以最终冻结应用重跑两布局，29 项通过。首轮失败报告和夹具保留，未覆盖为成功记录。

初次使用文件名作为 Rust 测试筛选时出现 0 项测试，未计为通过；最终 88 项使用真实模块名，包含 17 项 viewport。版本检查曾漏传参数，纠正调用后通过。一次报告读取因 Ruby 默认 US-ASCII 失败，显式使用 UTF-8 后成功，不影响应用验收。

首次旧浏览器夹具曾出现既有配置输入框 pattern 错误，未改动该无关源码；最终隔离页面未出现该错误，不据此宣称既有配置问题已修复。

## 隔离、清理与边界

验收使用私有 HOME/WILLDEEP_HOME/XDG/workspace、合成回环 Provider 和文件锁控制的真实 shell 命令。OSC52 仅捕获在私有 PTY，未写真实剪贴板。Web 使用合成历史展示，未执行其中的示例命令，也未调用真实模型。

两布局 PTY 的清理记录全部通过，成功夹具已清理。浏览器视口已恢复，预览已停止；使用该夹具环境执行 `daemon status` 返回 Runtime stopped，退出码 0。没有替换本机 CLI 或干扰另一个 LSP 修复会话的安装。

路径扫描覆盖中文或标点紧邻的绝对路径、`~/`、`./`、`../`、Windows 盘符、UNC、引号中的含空格路径以及 `file://`。其他协议 URL 保留。裸相对路径、无引号含空格路径、复杂引号转义未保证完整识别；因此工具契约同时禁止在说明中提供私有路径。完整命令详情仍保留路径并打码凭据。

本轮未验证真实模型是否每次遵循 label 契约，未重跑完整仓库测试、跨平台 CI、真实模型调用或 UI 版本本机安装。以上结果仅覆盖表中列明范围。

## 证据

- [结构化汇总](command-purpose-summary.json)
- [真实终端 JSON 报告](../../../target/command-purpose-0.154.0-rc1/final/pty-acceptance/tool-visuals.json)
- [真实终端 Markdown 报告](../../../target/command-purpose-0.154.0-rc1/final/pty-acceptance/tool-visuals.md)
- [Web HTTP 投影](../../../target/command-purpose-0.154.0-rc1/final/web-acceptance/web-projection.json)
- [首轮失败报告](../../../target/command-purpose-0.154.0-rc1/pty-acceptance/tool-visuals.json)
- [最终 Web 截图](command-purpose-web.jpg)
