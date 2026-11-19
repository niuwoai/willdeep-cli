# 0.154.0-rc5 q3 安装验收

日期：2026-11-19T11:25Z。结论：q3 由 rc3 升级到 `0.154.0-rc5`，willdeep 用户的 Runtime 与 Web listener 正常，root Runtime 保持停止，Tailscale 入口健康接口返回 rc5。机器可读记录见 [q3-installation.json](q3-installation.json)。

## 安装

| 项目 | 值 |
| --- | --- |
| 连接 | `q3-pub`（root） |
| 上一版本 | 0.154.0-rc3，两份二进制 SHA-256 `412e309e9ae2ab456486aee924f4a1850a6c5414ecdde3a136fd8d47bfb2def6` |
| 候选 | 官方 Release `willdeep-linux-amd64.tar.gz`，tar SHA-256 `4d5ff60d…6e5fd` |
| 新二进制 | `willdeep 0.154.0-rc5`，SHA-256 `4f20f7c9991f24d0ffd61c4f6689c73de33776f156608e420baad204ab84c2eb` |
| 安装位置 | `/usr/local/bin/willdeep`、`/home/willdeep/.local/bin/willdeep`，均为 willdeep:willdeep、0755 |
| 安装时间 | 2026-11-19T11:25:16Z |

安装前活动均为终态：任务 Completed 6、Cancelled 1；会话 Idle 2、Failed 1；人工等待为空。使用与 rc3 相同的维护 helper 流程：核对候选摘要与版本、备份两份二进制与三份 systemd 定义、校验空闲、原子替换、重启 `willdeep-daemon`，并在失败时自动恢复旧二进制。

## 安装后状态

| 组件 | 结果 |
| --- | --- |
| `willdeep-daemon.service` | willdeep、active/running、enabled，PID 3366213，版本 rc5 |
| `willdeep-web-listener.service` | willdeep、active/running、enabled，PID 3366228 |
| Runtime | `version=0.154.0-rc5`，`127.0.0.1:36095` |
| 健康接口 | `127.0.0.1:19847` 与 `127.0.0.1:9847` 的 `/health` 均为 200，`X-App-Version: 0.154.0-rc5` |
| root Runtime | 安装前后均 stopped |
| systemd 定义 | 三份文件摘要与安装前一致，未修改用户、命令、环境或监听 |
| Tailscale | `http://100.85.114.57:9847/health` 为 200，`x-app-version: 0.154.0-rc5`，body 版本 rc5 |

## 备份与回滚

备份目录：`/var/backups/willdeep/20261119-rc5/`。

- `0-willdeep-0.154.0-rc3`：原系统入口，SHA-256 `412e309e…`。
- `1-willdeep-0.154.0-rc3`：原 willdeep 用户入口，SHA-256 `412e309e…`。
- `installation.json`：安装前状态、两份二进制摘要、三份 unit 摘要与安装报告。
- helper：`/root/willdeep-rc5-20261119/q3-install-rc5.rb`。

回滚（helper 会先核对无活动任务/会话、人工等待为空与 root 停止，再恢复二进制、重启 daemon 并检查 rc3 健康）：

```bash
ssh q3-pub 'ruby /root/willdeep-rc5-20261119/q3-install-rc5.rb rollback /root/willdeep-rc5-20261119/willdeep 4f20f7c9991f24d0ffd61c4f6689c73de33776f156608e420baad204ab84c2eb /var/backups/willdeep/20261119-rc5'
```

本轮安装成功，未执行回滚。

## 边界

- 未读取 provider 配置、凭据或任务正文；未修改防火墙、Cloudflare、Tailscale、socat 或监听地址。
- 未启动 root Runtime；root 与 willdeep 的账号边界保持不变。
- 未使用真实模型；健康检查只验证本机 Runtime 与 Web 端点。
