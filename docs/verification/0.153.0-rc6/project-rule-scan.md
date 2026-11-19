# 项目规则扫描权限修复验收

- 日期：2026-11-19
- 版本：0.153.0-rc6
- 环境：macOS，非 root 进程
- 状态：本地源码验证；未安装或发布

## 结果与边界

不透明工具执行前，无法列举且无法进入的无关普通子目录可跳过，扫描继续发现其他规则。实际规则读取、已经加载的作用域、工作区根目录与越界符号链接继续保留阻断。禁止列举但仍允许按路径读取内容的目录不能跳过；新规则出现后仍须先呈现给模型，再重发工具调用。

完整边界见[设计决策](../../decisions/2026-11-19-project-rule-scan-permissions.md)。

## 验证经过

1. 在修改生产实现之前运行真实权限回归夹具，得到预期失败：`opaque_tools_skip_unreadable_unrelated_directories` 因 `.locked` 子目录的 `PermissionDenied` 失败，退出码 101。确认复现的是原始规则发现故障。
2. 初版通过 15 项规则测试后，独立复核发现目录 `0111` 时“无法列举但规则仍可读取”的遗漏。收紧到 `目录/.` 的 metadata 查询也明确被权限拒绝才允许跳过，并加入真实权限回归。
3. 最终实现的 16 项规则测试全部通过，0 失败、0 忽略。日志中没有权限夹具跳过提示；目录权限、规则文件权限和 `0111` 场景均实际执行。
4. 对最终实现运行核心库 `--all-targets` 严格 Clippy，通过且未放宽 warning。
5. 最终版本一致性、Rust 格式与差异空白检查通过。源码规模检查首次因本机 Ruby 默认 US-ASCII 无法处理文件名失败；使用 `ruby -EUTF-8` 重跑通过，无需修改检查脚本。
6. 最终核心库扩展测试通过：1361 通过、0 失败、7 项原有忽略，主动排除 1 项长时序测试，耗时 96.91 秒。排除项为 `agent::progress::tests::actual_goal_loop_escalates_repeated_errors_and_unchanged_successes`：包含实际退避等待，完整运行超过 4 分钟后中止，不记为通过。此前还曾因修复边界变化中止旧实现的完整测试；两次中止均不作为完整核心库通过证据。

## 复现命令

```bash
cargo test --offline --locked -p willdeep-core --lib project_rules::tests -- --nocapture
```

```bash
cargo clippy --offline --locked -p willdeep-core --all-targets -- -D warnings
```

```bash
cargo test --offline --locked -p willdeep-core --lib -- --skip agent::progress::tests::actual_goal_loop_escalates_repeated_errors_and_unchanged_successes
```

```bash
ruby scripts/check_release_version.rb 0.153.0-rc6
```

```bash
cargo fmt --all -- --check
```

```bash
ruby -EUTF-8 scripts/check_source_size.rb --output .claude/reports/rule-scan-unreadable-20261119/source-size
```

## 最终源码摘要

| 文件 | SHA-256 |
| --- | --- |
| `crates/willdeep-core/src/project_rules.rs` | `d7623514506b8febdb3c0843dd9e97b7548e001d294803a8d1efe73d526d9435` |
| `crates/willdeep-core/src/project_rules/tests.rs` | `e9d45f6fedcc7576d4c1762dcd313a7ad8661a12858f4a3c736e0715055957f4` |
| `crates/willdeep-core/src/project_rules/tests/permissions.rs` | `9e91df5740412732bf12112d9c592abf1b169df335c0c0083c38d7106f03b96a` |

## 未覆盖

未进行 Linux 原生测试或 q3 上的安装验证，未运行 Rust 全工作区及 Web 测试。此变更不涉及界面实现，未启动浏览器预览。没有修改真实私有目录权限、工作区选择或 systemd 服务配置。
