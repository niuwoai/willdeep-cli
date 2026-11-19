# 0.154.0-rc2 本机安装与发布前交接

日期：2026-11-19。按用户要求，先合并修复、更新本机并验证，再启动公开发布。

## 源码与构建

[规则扫描修复 PR 162](https://github.com/niuwoai/willdeep-cli/pull/162) 已合入默认分支 develop，合并提交为 `df5723085151b02095ee7c871f8bd374fc6201c6`。本机构建快照为 `ac91a4208d2fb699e4879f5a8c63951bd542635b`，与该合并提交的文件树相同；规则逻辑、回归与未覆盖范围见[源码验收](project-rule-scan.md)。

本 worktree 先构建 Web，再以独立 `.claude/target` 执行锁定依赖的 Rust release 构建及 `cargo install --offline --path crates/willdeep-cli --locked -j 4`。安装产物与构建产物摘要相同：

`72a55e6d15b9c227d81428c4f185a4ff3be7fa746057d40177bd7ae5e44cd05c`

Cargo 保留两项锁定依赖已被 registry 撤回的 warning：`chacha20 0.10.1`、`clipboard-win 5.4.1`。本次不放开锁文件或变更外部依赖；构建与安装退出码均为 0。

## 交接与真实检查

- Cargo 入口 `/Users/rocky/.cargo/bin/willdeep` 实际输出 `willdeep 0.154.0-rc2`。
- 使用 `willdeep daemon upgrade --timeout 300` 正常排空交接，未使用 `--force`。旧 Runtime 为 rc1、PID `22249`，新 Runtime 为 rc2、PID `61794`，控制端口 `127.0.0.1:55954`。
- CLI 版本、Runtime 状态、真实 capabilities RPC、Web `/health` 与版本头、首页及内嵌资源共 5 项检查通过。JSON 摘要见[机器可读结果](local-installation.json)。
- `http://127.0.0.1:9847/health` 返回 HTTP 200，正文版本与 `X-App-Version` 均为 `0.154.0-rc2`。
- 首页及 3 个关联 JS/CSS 资源均返回 HTTP 200；资源 SHA-256 与本 worktree 的 `web/dist` 一致。不能仅凭前端包版本推断实际服务已更新。
- 本记录时 Homebrew 入口仍为官方 rc1。GitHub Release、npm 和 Homebrew 更新需在后续发布步骤完成后另行记录，不能据此声明正式发行成功。
- 本次未部署 q3 或其他远端，没有数据库迁移。

## 备份与回滚

备份目录：`/Users/rocky/.local/share/willdeep/backups/20261119-rc1-before-0.154.0-rc2`。安装前已对两个备份分别与原文件逐字节比较，均一致。至少 6 份更早备份均保留，没有清理。

| 备份 | 原版本 | SHA-256 |
| --- | --- | --- |
| `cargo-willdeep` | 0.154.0-rc1 | `29299cb15eeea55d53ab20dd5a6b9b0fcf121b15773c9749e5af717a7a254a75` |
| `homebrew-willdeep` | 0.154.0-rc1 | `2228fb61781c264c009a28c9902957b6b8c3251ad8bc5d00b758634ec0366b29` |

以下为需要回滚时的命令，本次未执行。先处理活动任务及人工请求，再恢复 Cargo 入口并正常交接 Runtime：

```bash
test ! -e "$HOME/.cargo/bin/willdeep.restore-rc1" && install -m 755 "$HOME/.local/share/willdeep/backups/20261119-rc1-before-0.154.0-rc2/cargo-willdeep" "$HOME/.cargo/bin/willdeep.restore-rc1" && mv "$HOME/.cargo/bin/willdeep.restore-rc1" "$HOME/.cargo/bin/willdeep"
```

```bash
"$HOME/.cargo/bin/willdeep" daemon upgrade --timeout 300
```

Homebrew 入口尚未升级时无需回滚；发行后的 Homebrew 安装与恢复位置在最终发布记录中核对。已经打开的旧 TUI 不会随文件替换自动升级，重新启动客户端后使用新文件。
