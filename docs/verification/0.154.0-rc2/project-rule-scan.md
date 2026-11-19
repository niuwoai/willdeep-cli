# 项目规则扫描修复的基线合并验收

- 日期：2026-11-19
- 版本：0.154.0-rc2
- 环境：macOS，非 root 进程，独立 worktree；Node v24.8.0，Yarn 1.22.22
- 状态：合并快照的本地源码验证通过；本记录不代表安装或发布结果

## 合并范围

修复分支原提交为 `03315a4c099e3f5c3487be04811ac08981c878db`，初始候选版本为 0.153.0-rc6。获取远端 develop 后，将基线 `5cef108d64e31e48f8d441fb3f84322c2b1c5ab9` 合入该分支，保留 0.154.0-rc1 的 Provider CLI、Runtime WebApp 启动错误透传与 TUI 升级提示。

8 个冲突文件均为版本元数据、CHANGELOG 或产品/架构文档。当前版本统一顺延至 0.154.0-rc2，两个 Runtime crate 的版本钉和 npm 安装清单的 tag 同步。`Cargo.lock` 从基线恢复后由 `cargo update --offline --workspace` 重新生成，只有四个本地包版本变化，无外部依赖更新。

规则发现实现及两份测试源码与原修复提交逐字节一致；CLI 源码与合入的 develop 基线逐字节一致。权限边界仍见[设计决策](../../decisions/2026-11-19-project-rule-scan-permissions.md)。初始 rc6 的测试与失败经过保留在[历史验收记录](../0.153.0-rc6/project-rule-scan.md)，不作为本次合并快照的测试结果。

## 本次验证结果

- 规则扫描专项测试：16 通过、0 失败、0 忽略。没有 `SKIP permission fixture` 日志；普通目录、规则文件、已发现作用域、工作区根目录及 `0111` 目录的真实权限夹具均实际执行。
- 核心库扩展测试：1361 通过、0 失败、7 项原有忽略、1 项主动排除，耗时 100.51 秒。排除项为真实长退避测试，原因见未覆盖部分。
- CLI 基线回归：Provider CRUD 4 项、子命令解析 1 项、Runtime 托管 Web 5 项、TUI 升级通知 5 项，共 15 项通过，0 失败、0 忽略。
- Runtime client/protocol：67 项单元测试和 1 项文档测试通过，0 失败、0 忽略。
- Core 以及 Rust 全工作区 `--all-targets` 严格 Clippy 均通过，未放宽 warning。
- Web TypeScript 检查与 Vite 生产构建通过，未修改锁文件或依赖版本。保留已有的 chunk 超过 500 kB 提示。
- Web lint 通过：0 错误，保留 `ModelPicker.tsx:42` 的既有 `react-hooks/exhaustive-deps` warning，未扩大本修复范围处理该警告。Web 单元测试 52 个文件、432 项全部通过，0 失败。
- npm 安装与分发契约：7 项测试、38 个断言全部通过，0 失败、0 错误、0 跳过。
- `ruby scripts/check_release_version.rb 0.154.0-rc2`、Rust 格式、差异空白及规则修复相关的 10 个文档链接检查通过。
- 源码规模检查通过：1745 个手写文件，无超过 3000 行的违规。
- 独立静态合并复核未发现已核实缺陷；覆盖全部 8 个冲突文件、两边完整源码差异、发布元数据和历史/当前验收区分。该复核不替代原生测试。

所有本次 Rust 验证显式设置 `CARGO_TARGET_DIR` 为此 worktree 下的 `.claude/target`，不使用用户环境中的共享构建目录。

## 失败与纠正记录

两次在线 Web 安装在获取依赖阶段长时间未完成，主动停止，未记为构建通过。离线安装因缓存缺失失败（退出码 1），代表性输出：`Can't make a request in offline mode ("https://registry.npmmirror.com/lucide-react/-/lucide-react-1.55.0.tgz")`；该次未执行后续 build。随后以命令参数显式使用本机已有 HTTP 代理，锁定依赖安装在 8.83 秒内通过，生产构建通过。

最初以 `main_cli_tests::provider_leaf_commands_parse` 过滤时运行 0 项测试，不作为通过证据。纠正为实际测试名 `provider_leaf_commands_parse` 后，`tests::provider_leaf_commands_parse` 实际执行 1 项并通过。

## 复现命令

在 worktree 根目录执行，Rust 构建目录显式隔离：

```bash
CARGO_TARGET_DIR="$PWD/.claude/target" cargo test --offline --locked -j 4 -p willdeep-core --lib project_rules::tests -- --nocapture
```

```bash
CARGO_TARGET_DIR="$PWD/.claude/target" cargo test --offline --locked -j 4 -p willdeep-core --lib -- --skip agent::progress::tests::actual_goal_loop_escalates_repeated_errors_and_unchanged_successes
```

```bash
CARGO_TARGET_DIR="$PWD/.claude/target" cargo test --offline --locked -j 4 -p willdeep-runtime-client -p willdeep-runtime-protocol
```

```bash
CARGO_TARGET_DIR="$PWD/.claude/target" cargo test --offline --locked -j 4 -p willdeep --bin willdeep provider_cmd::tests
```

```bash
CARGO_TARGET_DIR="$PWD/.claude/target" cargo test --offline --locked -j 4 -p willdeep --bin willdeep provider_leaf_commands_parse
```

```bash
CARGO_TARGET_DIR="$PWD/.claude/target" cargo test --offline --locked -j 4 -p willdeep --bin willdeep daemon::web_app::tests
```

```bash
CARGO_TARGET_DIR="$PWD/.claude/target" cargo test --offline --locked -j 4 -p willdeep --bin willdeep tui::update_notice::tests
```

```bash
CARGO_TARGET_DIR="$PWD/.claude/target" cargo clippy --offline --locked -j 4 --workspace --all-targets -- -D warnings
```

以下安装命令中的回环代理是本机已有服务，其他环境按实际网络配置运行：

```bash
yarn --cwd web install --frozen-lockfile --network-timeout 30000 --network-concurrency 8 --proxy http://127.0.0.1:7890 --https-proxy http://127.0.0.1:7890
```

```bash
yarn --cwd web build
```

```bash
yarn --cwd web lint
```

```bash
yarn --cwd web test
```

```bash
ruby scripts/test/npm_willdeep_test.rb
```

## 源码摘要

| 文件 | SHA-256 |
| --- | --- |
| `crates/willdeep-core/src/project_rules.rs` | `d7623514506b8febdb3c0843dd9e97b7548e001d294803a8d1efe73d526d9435` |
| `crates/willdeep-core/src/project_rules/tests.rs` | `e9d45f6fedcc7576d4c1762dcd313a7ad8661a12858f4a3c736e0715055957f4` |
| `crates/willdeep-core/src/project_rules/tests/permissions.rs` | `9e91df5740412732bf12112d9c592abf1b169df335c0c0083c38d7106f03b96a` |

## 未覆盖

没有进行 Linux 原生或 q3 安装验证，没有修改真实私有目录权限、工作区选择或 systemd 配置。核心库测试继续排除包含实际 1/5/15 分钟退避的 `agent::progress::tests::actual_goal_loop_escalates_repeated_errors_and_unchanged_successes`；初始 rc6 运行中止的经过保留在历史验收，不记为通过。未运行 Rust 全工作区测试。本次冲突解决未修改浏览器交互实现，不启动无法证明权限逻辑的浏览器预览。
