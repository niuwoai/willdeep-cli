# 0.152.0-rc4 本机安装与 Runtime 交接

日期：2026-11-17。结果：两个本机 CLI 入口与常驻 Runtime 均为 rc4，正常交接及健康查询通过，未公开发布。机器可读记录见 [JSON](local-installation.json)，对应 [PR](https://github.com/niuwoai/willdeep-cli/pull/157)，最终优化应用与真实交互结果见[验收报告](README.md)。

## 安装与实际核验

先构建 rc4 Web 资源及 release 应用，最后两处 Rust 显示修正后再次优化构建，并将应用冻结到 `.claude/scratch/tui-rc4-final2/willdeep`。该副本在私有 HOME/工作区中完成 28+18+10 项真实 PTY 检查后，直接安装同一份文件，不通过另一次构建改变安装应用身份。

替换前再次将两个旧入口与各自 rc3 备份逐字节比较，确保未覆盖其他会话的新安装。先将最终应用安装到各入口旁的新文件，再原子改名替换；保留 Homebrew 符号链接。

| 入口 | 实际文件 | 版本 |
|---|---|---|
| `~/.cargo/bin/willdeep` | 同入口 | `0.152.0-rc4` |
| `/opt/homebrew/bin/willdeep` | `/opt/homebrew/Cellar/willdeep-cli/0.149.0-rc12/bin/willdeep` | `0.152.0-rc4` |

两个入口的 SHA256 均为 `a776238e14831a97592c56704e3d56945ddb6500ea48ad0fde1eb99ac7511e44`，并再次与最终验收副本执行逐字节比较，通过。Homebrew 这里只做本机试用覆盖，未修改发行公式或远程 tap；后续 Homebrew 管理操作可能替换本次覆盖。

执行 `~/.cargo/bin/willdeep daemon upgrade --timeout 300`，退出 0；Runtime 从 rc3/PID 96806 正常交接到 rc4/PID 25984。两个入口查询到同一 Runtime：`running`、版本 `0.152.0-rc4`、地址 `127.0.0.1:51433`。`ps` 和 `lsof` 确认实际执行文件为 `~/.cargo/bin/willdeep`。本次未使用强制重启，未取消任务。本次未新增数据库迁移，也未执行数据库 down。

## 试用

已经启动的 TUI 仍运行旧程序。先保留未发送草稿，退出后重新打开：

```bash
willdeep
```

## 备份与回滚

rc3 备份目录：`~/.local/share/willdeep/backups/20261117-rc3-before-rc4/`，文件为 `cargo-willdeep` 和 `homebrew-willdeep`。安装前与旧入口比较一致，安装后重新核对备份 SHA256 仍为 `69e161f57d6403ebb8838d7252384ad6e2e8d32961568e790357e29e3dde7ce1`。既有其他版本备份未清理。

以下命令是需要回滚时的操作说明，本次没有执行。先处理活动任务和人工请求，再恢复两个入口并正常交接 Runtime，不以强停绕过等待。

```bash
test ! -e "$HOME/.cargo/bin/willdeep.restore-rc3" && install -m 755 "$HOME/.local/share/willdeep/backups/20261117-rc3-before-rc4/cargo-willdeep" "$HOME/.cargo/bin/willdeep.restore-rc3" && mv "$HOME/.cargo/bin/willdeep.restore-rc3" "$HOME/.cargo/bin/willdeep"
```

```bash
test ! -e /opt/homebrew/Cellar/willdeep-cli/0.149.0-rc12/bin/willdeep.restore-rc3 && install -m 755 "$HOME/.local/share/willdeep/backups/20261117-rc3-before-rc4/homebrew-willdeep" /opt/homebrew/Cellar/willdeep-cli/0.149.0-rc12/bin/willdeep.restore-rc3 && mv /opt/homebrew/Cellar/willdeep-cli/0.149.0-rc12/bin/willdeep.restore-rc3 /opt/homebrew/Cellar/willdeep-cli/0.149.0-rc12/bin/willdeep
```

```bash
willdeep daemon upgrade --timeout 300
```

该版本仅本机试用。未执行默认分支合并、GitHub Release、npm/Cargo 发布或发行 tag；发布 tag 须等提交合入默认分支。
