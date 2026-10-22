# macOS Rust CLI 流式显示卡顿修复验收

日期：2026-10-22。基线：已安装的 `0.113.0-rc2`，源代码基线 `3247253`。修复候选：`0.113.0-rc3`，分支 `codex/fix-stream-display-latency`。

## 定位结论

瓶颈位于 `tui/runtime_ui.rs::apply_runtime_events`：原实现为每个 Runtime 事件同步读取整份 Core Session，并通过 `SessionStore::update` 再次读取、序列化、写临时文件、同步文件、替换文件并同步目录。操作发生在 TUI 绘制之前，并与 Runtime 流式检查点争用同一把会话锁。其他会话的事件同样会触发这条路径。

受控流每 20 ms 发送一个正文增量。2000 条合成历史下，旧版一次对照的 Runtime 增量到达中位数为 6.181 ms，而屏幕显示中位数为 1035.696 ms；实时 NDJSON 端点返回 200。另一次旧版在没有编译负载时重测，显示中位数为 2153.687 ms。相同历史绕过 Runtime 跟随、直接进本地 TUI 时中位数为 0.695 ms。由此确认存在客户端逐片落盘造成的排队；本次复现未触发旧 Runtime 的 HTTP 404 / 一秒轮询兼容回退。

## 修复行为

- 正文与思考增量只更新当前预览和内存事件游标，不读取或重写整份会话。
- 其他会话的事件只推进内存游标，不触发当前会话读写。
- 定稿、工具、轮次等当前会话事件仍在会话锁内合并最新 Runtime 历史、游标和已读标记；正常退出时冲刷剩余游标。
- Runtime 原有 250 ms / 16 KiB 流式检查点、事件日志、错误与审批处理保持原实现；恢复无需重新请求模型。

## 真实链路测量

链路：本地固定节奏 SSE Provider → 真实 Rust Provider 解码 → 真实 Runtime / 检查点 / 持久事件流 → 真实 TUI → PTY 输出 → `@xterm/headless` 按每次 PTY 读取的时间重建画面。

每次测量使用隔离的 `WILLDEEP_HOME`、合成项目和 2000 条合成历史，发送 160 个包含中文的唯一标记，每 20 ms 一段。独立订阅 Runtime NDJSON 记录各增量到达时间，并检查一轮推理、完整答案落盘、实际完成画面、Ctrl+C 正常退出和游标冲刷。请求均为 Chat Completions，所有模型流量只到本机回环地址。

| 构建与场景 | 显示中位数 ms | 显示 P95 ms | 及时显示标记 |
| --- | ---: | ---: | ---: |
| 旧版 Runtime，首次压力复现 | 734.698 | 3484.061 | 134 / 160 |
| 旧版 Runtime，独立事件观察 | 1035.696 | 1754.247 | 160 / 160 |
| 旧版 Runtime，编译结束后重测 | 2153.687 | 2701.529 | 160 / 160 |
| 旧版 `/local` 路径对照 | 0.695 | 18.626 | 160 / 160 |
| 修复版 Runtime，简洁 UI 第 1 轮 | 5.544 | 17.306 | 160 / 160 |
| 修复版 Runtime，简洁 UI 第 2 轮 | 13.030 | 78.244 | 160 / 160 |
| 修复版 Runtime，简洁 UI 第 3 轮 | 5.520 | 19.472 | 160 / 160 |
| 修复版 Runtime，完整 UI | 5.326 | 17.316 | 160 / 160 |

修复版四轮全部通过 P95 ≤ 200 ms 的门槛。流生成期间观察到的会话文件更新次数均为 14，主要来自保留的 Runtime 检查点；旧版相同阶段观察到 73–113 次更新。mtime 采样可能合并多次写入，此值不等同精确的系统调用计数。

修复版完整 UI 与简洁 UI 的全部 640 个标记都进入了终端画面，每轮完整答案和正常退出游标均正确保存。以上证明受控链路上的客户端延迟改善，不是 some.im 线上推理吞吐或 QTerm 显示帧率的测量。

## 回归与构建

- 新增 4 项回归：正文/思考增量不重写会话、其他会话事件不重写本地历史、定稿合并不覆盖新检查点、正常退出保存游标和部分答案。
- 全部 CLI 单元：826 通过、0 失败、3 项原有手工测试忽略；其中 TUI 286 通过。
- 版本一致性集成检查：1 通过。`cargo fmt --all -- --check`、`git diff --check` 通过。
- Web 嵌入产物与 macOS arm64 Release 候选均构建成功。测试使用优化构建；未优化 Debug 构建有额外排版/调度开销，不用于这里的性能结论。
- 未推送、发布或替换本机安装版，现有 Runtime 和用户会话没有重启。

完整测量摘要与原始证据目录见 [JSON 报告](streaming-latency.json)。候选二进制保存于本工作树 `target/streaming-0.113.0-rc3/willdeep`，SHA-256 为 `e7eacc25156c75f5a8e0415830c145571947ad81c335dbeb609ecef072065ff1`。

## 复跑

先构建 Web 与 CLI，确保 `web/node_modules` 中已安装 `@xterm/headless`。执行：

```sh
STREAM_MAX_P95_MS=200 ruby scripts/stream_latency_tui_verify.rb /绝对路径/willdeep /private/tmp/stream-report 2000 runtime
STREAM_MAX_P95_MS=200 STREAM_UI_MODE=full ruby scripts/stream_latency_tui_verify.rb /绝对路径/willdeep /private/tmp/stream-full-report 2000 runtime
```

脚本生成 JSON 与 Markdown 报告，保留带时间戳的 PTY 原始数据和合成会话。可用第四个参数 `local` 对照进程内路径；不会读取真实模型凭据或用户会话。
