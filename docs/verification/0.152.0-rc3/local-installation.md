# 0.152.0-rc3 本机安装与 Runtime 交接

日期：2026-11-17。结果：本机安装及健康检查通过，未发布。对应源码提交 `aff3f6b6f787ac8858ab84bd252bdba7f3b0b33e`，PR 为 [niuwoai/willdeep-cli#157](https://github.com/niuwoai/willdeep-cli/pull/157)。机器可读记录见 [JSON](local-installation.json)，提交前工程与真实入口验收见[主报告](p0-p1-closeout.md)。

先执行 `yarn --cwd web build`，再执行 `cargo install --offline --path crates/willdeep-cli --locked --force`，两步均退出 0。优化构建耗时 4 分 22 秒。CLI 安装本身不构建前端，因此先更新忽略的 `web/dist`，确保嵌入最新页面。

## 实际状态

- `~/.cargo/bin/willdeep` 与 `/opt/homebrew/bin/willdeep` 均返回 `willdeep 0.152.0-rc3`，两处二进制 SHA256 相同：`69e161f57d6403ebb8838d7252384ad6e2e8d32961568e790357e29e3dde7ce1`。
- Homebrew 命令入口使用本机试用覆盖，实际文件仍位于 `Cellar/willdeep-cli/0.149.0-rc12/bin/willdeep`。没有更新发行公式或远程 tap；后续 Homebrew 管理操作可能替换本次本机覆盖。
- 本机原 Runtime 存在一项 WaitingApproval。用户明确选择「取消旧任务并升级」后，仅取消该任务，确认 Cancelled 与人工请求为空，再执行 `willdeep daemon upgrade --timeout 300`，退出 0。
- Runtime 从 rc12/PID 80906 平滑交接到 rc3/PID 96806；实际执行路径为 `~/.cargo/bin/willdeep`。健康查询为 running、版本 rc3，旧任务取消状态在交接后仍保留，待审批/提问为空。
- `run --help` 包含 `--continue-task`；安装完成后源码摘要仍为 `b1a7b55af7ae8ac297c001e1ced1f531b98b1777f19342b2098aab397e076d58`。

本次安装是重建 Web 资源后的 release 优化构建，其 SHA 与主报告中固定的 debug 验收应用不同。主报告的工程、67 项 PTY 与 8 项自定义智能体结论保留原始应用身份，不冒充对本机优化二进制的全量重跑。本机核验覆盖安装、版本、命令入口、续跑选项、实际 Runtime 执行路径、健康状态与交接结果，未调用模型或搜索。

安装日志有锁文件中 `chacha20 0.10.1`、`clipboard-win 5.4.1` 已撤回的 Cargo 警告，以及 Vite 大于 500 kB 的 chunk 警告；构建与安装均退出 0，未为消除警告改变依赖或锁文件。

## 试用

已有 TUI 进程仍使用启动时的旧程序，请先保留未发送草稿，退出后重新打开：

```bash
willdeep
```

## 备份与回滚

替换前分别备份旧 Cargo 和 Homebrew 二进制，逐字节摘要校验一致。目录为 `~/.local/share/willdeep/backups/20261117T103010Z-before-0.152.0-rc3`，文件分别为 `cargo-willdeep` 和 `homebrew-willdeep`，均为旧 rc12。

以下命令只供需要回滚时使用，本次没有执行。先处理新任务及人工请求，再原子替换两个入口，最后通过正常 Runtime 交接回旧版；不要通过强停来丢弃新任务。

```bash
install -m 755 "$HOME/.local/share/willdeep/backups/20261117T103010Z-before-0.152.0-rc3/cargo-willdeep" "$HOME/.cargo/bin/willdeep.restore-rc12" && mv "$HOME/.cargo/bin/willdeep.restore-rc12" "$HOME/.cargo/bin/willdeep"
```

```bash
install -m 755 "$HOME/.local/share/willdeep/backups/20261117T103010Z-before-0.152.0-rc3/homebrew-willdeep" /opt/homebrew/Cellar/willdeep-cli/0.149.0-rc12/bin/willdeep.restore-rc12 && mv /opt/homebrew/Cellar/willdeep-cli/0.149.0-rc12/bin/willdeep.restore-rc12 /opt/homebrew/Cellar/willdeep-cli/0.149.0-rc12/bin/willdeep
```

```bash
willdeep daemon upgrade --timeout 300
```

二进制回滚不会撤销用户已授权的旧任务取消。未进行数据库 down、公开 Release、发行 tag 或默认分支合并。
