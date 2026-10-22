# 0.114.0-rc2 最终验收

日期：2026-10-22。macOS 优化版 CLI，独立临时工作区与 WILLDEEP_HOME，真实 Runtime、PTY 和 xterm 终端重建；回环服务只接收合成数据。

发布目标：`v0.114.0-rc2`，保留独立流式修复与选择器功能提交；GitHub 四平台发行包、Homebrew、Cargo 安装及 Runtime 交接分别核验。

## 流式瓶颈与修复

1. TUI 原先每个预览片段读写完整会话；现只更新内存，Runtime 保存检查点，轮次结束和正常退出时合并客户端游标。
2. Runtime 原先每个片段同步落盘，阻塞 Provider 接收。专用线程用最多 16ms / 64 条的窗口合并写入和 sync_data；持久化成功后才广播，关键事件同步等待，读取与退出先刷新队列。失败不推进已提交序号、不广播。
3. 实时订阅原先每个片段重扫整个历史日志。连续事件直接从广播队列读取，缺口或队列落后时先补持久日志。
4. 新 TUI 首次提交时可能先读取尚未启动的 Runtime，取得零游标；Runtime 启动后从零重放整个日志。现启动后取得日志头，重建首次订阅，并合批处理排队事件；已有非零游标保持断点恢复。

聊天消息与模型上下文按会话隔离。Runtime 的 events.ndjson 是带会话归属的全局事件日志；历史重放积压不等于把别的会话发给模型。

## 实际到屏延迟

固定 2,000 条聊天历史、20,000 条其他会话历史事件，每 20ms 发送一个标记，共 160 个；从服务端发送时刻量到终端重建后的可见时刻，门槛 P95 ≤200ms，且完整回答与退出游标均须保存。

| 场景 | 到屏 | 中位延迟 | P95 | 最大延迟 | 最大显示间隔 |
|---|---:|---:|---:|---:|---:|
| 首次订阅修复前，简洁 UI | 160/160 | 70,164.602ms | 75,733.067ms | 75,766.531ms | — |
| 最终版，简洁 UI | 160/160 | 24.814ms | 49.016ms | 75.803ms | 62.160ms |
| 最终版，完整 UI | 160/160 | 24.602ms | 65.879ms | 185.458ms | 134.129ms |

修复前 Runtime 新事件本身 P95 89.699ms，延迟主要出现在 TUI 历史重放；修复后两模式 Runtime 新事件 P95 分别 42.232ms / 65.015ms。两次最终测试都正常退出，完整答案保存、游标刷新、HTTP 200；观察到的 14 次会话重写来自 Runtime 检查点，不再逐片重写。

复跑：

```sh
STREAM_JOURNAL_EVENTS=20000 STREAM_MAX_P95_MS=200 STREAM_UI_MODE=compact ruby scripts/stream_latency_tui_verify.rb /path/to/willdeep /tmp/stream-compact 2000 runtime
STREAM_JOURNAL_EVENTS=20000 STREAM_MAX_P95_MS=200 STREAM_UI_MODE=full ruby scripts/stream_latency_tui_verify.rb /path/to/willdeep /tmp/stream-full 2000 runtime
```

原始 JSON / Markdown 保存于 `/private/tmp/willdeep-release-0.114.0-rc2/`；失败也保存报告。此前 rc1 的偶发长尾保留在 [原验收记录](../0.114.0-rc1/README.md)，未用单次快结果掩盖。这里验证客户端与 Runtime 链路，不据此断言 some.im 上游所有模型都快。

## 功能与回归

- CLI 单测：846 通过、0 失败、3 项原有忽略项；覆盖分组提交、单片延迟、读取与退出刷新、磁盘失败不广播、实时不扫日志、序号缺口补齐、首次订阅及活动轮次不写盘。
- workspace / all-target Clippy、cargo fmt、Web 0.114.0-rc2 构建与文件尺寸边界通过。
- 最终优化版两栏选择器：18 步通过，键盘、鼠标、缓存、窄窗口、取消与实际 Provider / 模型一致性均验证。
- 最终优化版跨协议 Provider 切换：OpenAI → Anthropic → 鼠标切回，前台、Runtime 和两次子 Agent 请求认证与模型正确。
- 最终优化版本地发现：11 步通过，启动发现 Magpie、运行后发现 Ollama、分别确认模型并获得 Runtime 回答、重开无重复项且默认 Provider 保留；探测请求没有认证头。PTY 清理添加有界退出等待，避免测试收尾无限等待。
- 本地发现的范围、地址安全约束、配置保留与 Favorites 2.3.0 验证方法见 [功能验收](../0.114.0-rc1/README.md)。发布与本地安装结果另行核对，不以构建通过代替交付完成。
