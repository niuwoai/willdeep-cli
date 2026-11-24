# CI 计时与缓存验证（合入 0.155.0-rc11）

日期：2026-11-24

基线：`e7d29f31`（develop，含 rc2 与 PR 自建 runner 调度修复）。

工作分支：`fix/ci-test-speed`。

## 实施范围

- core 的 Tokio `test-util` 仅作为开发依赖，目标续跑无进展阶梯测试使用暂停时钟。
- 保留原有工具调用及阶梯断言，额外验证重复错误场景 60 + 300 秒、重复成功场景 60 秒退避，合计推进 420 秒虚拟时间。
- 完整 CI 套件和 WSL release 构建增加 Rust 依赖缓存，区分平台、CPU 架构和构建用途；仅 develop/main 保存，不缓存安装工具及工作区 crate，不保存失败构建。
- 本地验证时版本曾同步为 rc3；合入当前 develop 时保留已发布的 0.155.0-rc11，不重复提升版本。

生产退避源码未改，平台矩阵、现有测试命令、Clippy、打包检查和失败门禁保持不变。原工作区并行改动未触碰。

## Rust 验证

以下测试及严格 Clippy 在同步 rc3 元数据之前、使用 rc2 包身份完成；测试源码与开发依赖配置此后未改。版本同步后执行离线锁文件解析、版本一致性及全工作区格式检查，没有为纯版本变化重复冷编译测试二进制。

所有编译与测试使用本 worktree 的独立 `target`，没有复用或修改原工作区共享 target。

| 检查 | 结果 | 测试自身耗时 |
| --- | --- | ---: |
| `agent::progress::tests::actual_goal_loop_escalates_repeated_errors_and_unchanged_successes` | 1 通过，0 失败 | 0.54 秒 |
| `agent::progress::tests::` | 6 通过，0 失败 | 0.49 秒 |
| `goal::tests::backoff_waits_one_then_five_then_fifteen_minutes` | 1 通过，0 失败 | 0.02 秒 |
| `cargo test --locked -p willdeep-core --lib` | 1404 通过，0 失败，7 忽略 | 146.08 秒 |
| `cargo clippy --locked -p willdeep-core --lib --tests -- -D warnings` | 通过，退出码 0 | 不适用 |
| `cargo fmt --all -- --check` | 通过 | 不适用 |

定向测试的首次冷编译耗时 10 分 46 秒，命令总 real 651.22 秒；这包含独立构建成本，不能当成测试执行耗时。progress 组命令 real 465.36 秒、Clippy 命令 real 518.20 秒均包含排队等待同一 target 的构建锁。core 全量回归复用构建耗时 0.30 秒，命令 real 146.46 秒。

全量回归 146.08 秒是本机样本；既有 GitHub core 组 421–423 秒来自其他运行环境，不据此承诺整组或完整 CI 的固定提速比例。可确认的是目标测试的 420 秒真实等待已经由虚拟时间推进，全部时序断言执行通过。

## 其他本地检查

- npm 安装与发行契约：7 + 14 项测试，38 + 94 条断言，0 失败、0 错误、0 跳过。
- 源码大小边界：通过，检查时 1808 个文件，无超过 3000 行的违规文件。
- 源码大小检查初次因当前 Ruby 默认 US-ASCII 编码报 `ArgumentError: invalid byte sequence in US-ASCII`；使用 `ruby -E UTF-8:UTF-8` 重跑通过。未修改该脚本。
- `cargo update --offline --workspace` 只更新 4 个工作区包的版本，未变更外部依赖。
- `ruby scripts/check_release_version.rb 0.155.0-rc3`：通过。
- `cargo metadata --locked --offline --no-deps --format-version 1`：通过，4 个工作区包版本一致，target 路径为本 worktree 的独立目录。

- CI runner 离线契约：41 项测试、563 条断言，0 失败、0 错误、0 跳过；只用模拟 API 和时钟，没有派发远程工作流。原始 JSON/Markdown 报告位于本 worktree 的 `target/ci-cache-contracts/report.json`、`report.md`。
- YAML 解析与 actionlint 通过。独立复核使用 actionlint 时关闭 ShellCheck；本轮没有修改 shell 命令。
- 将两个 YAML 中新增的缓存步骤移除后，解析结果与基线 HEAD 完全一致，确认未调整原有任务、条件或验证顺序。
- 计时修改和缓存修改分别由独立只读复核检查，均未发现需要修复的正确性问题；复核涵盖上游 action 输入及键生成/保存逻辑。共享键不保证不同环境命中相同缓存，action 仍校验工具链和编译环境指纹。
- 最终 `git diff --check` 通过，生产 `agent.rs` 与 `goal.rs` 没有差异。

机器可读汇总见同目录 `ci-test-speed.json`。

## 尚未验证

- GitHub Actions 实际缓存恢复、保存和容量变化。首次可写入基线须等主线成功构建；PR 没有可访问基线时继续冷编译。
- 托管及自建 runner 的冷启动与缓存命中耗时对比。
- 完整跨平台 CI 与整个 Rust workspace 测试；本轮 Rust 回归范围为 core lib 和其测试目标 Clippy。

未提交、推送、安装或公开发布。本地结果不等于 GitHub 总耗时收益。
