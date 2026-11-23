# 摘要恢复修复与最新 develop 集成验收

日期：2026-11-23。候选版本：`0.154.0-rc11`。PR：[niuwoai/willdeep-cli#176](https://github.com/niuwoai/willdeep-cli/pull/176)。

## 集成范围

初始修复提交为 `34c875a0`，版本为 rc8，原始测试与失败历史保留在 [rc8 验收快照](../0.154.0-rc8/README.md)。该快照的“尚未提交”与机器报告 `committed: false` 描述当时状态。

PR 创建后，发现最新 develop 已到 `9ca1d7ab`（rc10），包含 TUI 输入归属修复及两个 CI 测试夹具修复。本次合入该提交，保留上游 rc8–rc10 更新记录，将版本顺延至 rc11，并同步 Rust、Web、npm、产品概览、架构与决策文档。

冲突均在版本或发布文档；TUI 源码自动合并，Core 摘要恢复与 Harness 实现没有合并变更。独立只读审查未发现 TUI 自动合并新增的确定性行为问题；未知斜杠、已知命令参数兜底以及人工弹层的输入优先级均保留。`app_state.rs` 为 2912 行，未超过 3000 行。

## 验证状态

原生集成验证及两套真实终端验收均通过。

| 检查 | 结果 |
| --- | --- |
| `cargo test -p willdeep --bin willdeep tui:: -- --test-threads=6` | 712 通过、0 失败、2 忽略 |
| `cargo test -p willdeep --test workspace_versions` | 1 通过 |
| `cargo clippy --workspace --all-targets -- -D warnings` | 通过 |
| `cargo fmt --all -- --check`、`git diff --check` | 通过 |
| `ruby scripts/check_release_version.rb 0.154.0-rc11` | 通过 |
| `ruby -c scripts/verify_unknown_slash_tui.rb` | 通过 |
| `yarn --cwd web build` | 通过 |
| `cargo build -p willdeep --bin willdeep` | 通过，实际版本 `willdeep 0.154.0-rc11` |
| rc11 实际二进制的未知斜杠输入 PTY | 4/4 通过；3 次假模型请求、0 次真实模型请求，退出清理无错误 |
| rc11 实际二进制的 full/compact 输入归属 PTY | 15/15 通过（14 项交互及 1 项二进制 SHA256 稳定检查），两种布局的 10 项清理检查均通过 |

Cargo 使用工作树独立的 `target/`，构建并发为 6，未使用共享构建目录。

真实终端执行入口：

```bash
ruby scripts/verify_unknown_slash_tui.rb target/debug/willdeep target/rc11-slash-tui
```

```bash
ruby scripts/verify_tui_input_ownership.rb target/debug/willdeep target/rc11-input-ownership
```

原始 JSON、Markdown、ANSI、逐格终端帧及夹具合成请求保留在被忽略的 `target/rc11-slash-tui/` 与 `target/rc11-input-ownership/`，不纳入公开提交；可公开结果汇总见 [JSON](report.json)。临时工作区仍保留供排查，清理通过指夹具拥有的进程与连接正常退出。

Rust 首次验证命令错误指定了 `cargo test -p willdeep --lib`，因 CLI 只有二进制目标而退出：`error: no library targets found in package willdeep`。已改为 `--bin willdeep` 执行；这是验证命令错误，未改动生产逻辑。

## 边界

原 rc8 核心摘要、检查点、规则与重试回归结果沿用原验收快照；不把它们记为 rc11 全套重跑。真实终端验收使用私有 HOME、工作区、Runtime 及 loopback 假模型，不读取真实会话或凭据。候选尚未安装、公开发布或合入 develop。
