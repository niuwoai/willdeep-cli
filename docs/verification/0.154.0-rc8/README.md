# 0.154.0-rc8 压缩恢复与未知斜杠输入验收

日期：2026-11-23。分支：`fix/session-compaction-resume`。既有项目修复候选。本报告记录提交前的验收快照；验收时尚未提交、推送、安装或公开发布。

## 验证结论

所有受影响测试组、严格静态检查、版本一致性检查和真实 TUI 夹具最终通过。已有摘要在模型失败或取消后保留；重建执行器与 Store 后可复用，实际规则或历史前缀变化仍使摘要失效。联邦通知插回消息列表时，检查点、Runtime 首轮、后台续轮与子任务成功结果共用摘要边界映射。

压缩采用一次操作 120 秒总预算、每候选最多 45 秒，并为多个分块、归并、二次摘要共用预算；测试使用短时限假 Provider 验证机制，不调用真实模型验证压缩质量或生产耗时。

## 最终执行结果

| 检查 | 结果 | 覆盖范围 |
| --- | --- | --- |
| Core `agent::context` | 68 通过 | 自动与手动摘要复用、规则变更、取消、预算、分块与归并、计费 |
| Core `checkpoint::` | 68 通过、1 忽略 | 主模型前保存、模型错误/取消后恢复、写前/写后失败、索引映射与执行所有权 |
| Core `project_rules::` | 20 通过 | 历史作用域恢复、不重执行工具、当前规则更新、路径边界和符号链接 |
| Core `provider::common::retry_tests` | 7 通过 | 退避、总期限、重试次数及诊断字段 |
| Core `subagent::checkpoint` | 6 通过 | 新执行器复用，以及成功收尾合并重复通知后仍复用 |
| CLI `harness::` | 87 通过、1 忽略 | Runtime Harness、检查点集成及已有手动压缩等行为 |
| CLI `tui::` | 691 通过、2 忽略 | 命令、普通输入、附件、队列、回显和本地摘要保存 |
| `workspace_versions` | 1 通过 | 内部 crate 版本依赖与 workspace 一致 |
| `cargo clippy --workspace --all-targets -- -D warnings` | 通过 | 最终代码，包括共享保存入口与新增测试 |
| `cargo fmt --all -- --check` / `git diff --check` | 通过 | 格式与空白检查 |
| `ruby scripts/check_release_version.rb 0.154.0-rc8` | 通过 | Rust、Web、npm 和文档版本元数据 |
| Web 离线依赖安装与构建 | 通过 | 为 RustEmbed 提供独立 worktree 的 `web/dist` |
| `cargo build -p willdeep --bin willdeep` | 通过 | 真实 CLI 显示 `willdeep 0.154.0-rc8` |
| Ruby PTY 真实 TUI 验收 | 4/4 通过 | 绝对路径、未知命令、命令名子路径、已知命令非法参数 |

测试筛选存在重叠，尤其 `checkpoint::` 包含子任务检查点测试；不将各行相加作为独立测试总数。忽略项沿用已有测试配置。

## 真实 TUI 证据

执行：

```bash
ruby scripts/verify_unknown_slash_tui.rb target/debug/willdeep
```

夹具启动真实 TUI 和 Runtime，使用临时工作区、临时 WILLDEEP_HOME、HOME、USERPROFILE、XDG_CONFIG_HOME，关闭本机模型发现，连接随机 loopback 端口的假模型。最终收到 3 次假模型请求，真实模型请求 0 次。

- `/tmp/slash-fixture/project Please inspect this directory` 原样进入模型。
- `/wat Please treat this as ordinary input` 原样进入模型。
- `/help/file Please inspect this file` 原样进入模型。
- `/verbose on` 在本地显示参数用法，不新增模型请求。

[机器报告](slash-tui/report.json)与[简明报告](slash-tui/report.md)记录最终完成且清理错误为零的运行结果。原始 ANSI 终端记录仅用于本地排查，不纳入公开提交。夹具的临时目录保留供排查；“清理错误为零”指进程退出和套接字关闭无报错，不表示已删除临时文件。

## 失败历史与修正

- 独立 worktree 首次 CLI 测试编译失败：缺少 RustEmbed 必须的 `web/dist`。离线构建 Web 后解决。
- 新增规则测试首次编译失败：`Message::assistant` 缺少工具调用参数；已修正。
- 核心库首次完整执行为 1398 通过、2 失败、7 忽略。失败分别是旧测试仍要求恢复时仅加载根规则，以及编译时尚未包含调整的“准备失败保留旧摘要”测试。更新契约测试后，压缩、检查点、规则和重试组全部通过。
- 审查进一步发现成功收尾会覆盖已映射的摘要边界；提取共享保存入口，补充真实子任务 wrapper 回归，并重跑检查点、子任务、Harness、TUI 与严格 Clippy。
- 完整核心库未在最后改动后整体复跑；最终以受影响测试组和工作区静态检查验证，未声称最后一次全库执行全部通过。
- 终端夹具在隔离用户目录后暴露 macOS PTY 退出等待：4 项交互已完成，但清理挂起。旧夹具被终止，另一次运行明确因清理超时失败；调整为先关闭 PTY、停止隔离 Runtime，并对所有退出等待设上限后，最终 4/4 通过且清理错误为零。

## 交付边界

未读取真实会话验证生产现场；无法据此确定某次上游请求具体慢在哪个阶段。120 秒是压缩等待预算，不是任意历史都能在两分钟内成功压缩的承诺。候选二进制仅位于此 worktree 的 `target/debug`，未替换用户正在使用的 CLI 或 Runtime，也未公开发布。
