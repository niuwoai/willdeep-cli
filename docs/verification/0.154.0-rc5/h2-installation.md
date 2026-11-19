# 0.154.0-rc5 h2.niuwoai.com 安装验收

日期：2026-11-19T11:27Z 前后。结论：h2 由 `0.149.0-rc2` 升级到 `0.154.0-rc5`，使用 willdeep 用户目录与官方 Linux amd64 产物，无需 sudo；Runtime 通过自交接升级到 rc5。机器可读记录见 [h2-installation.json](h2-installation.json)。

## 安装

| 项目 | 值 |
| --- | --- |
| 连接 | `h2.niuwoai.com`（willdeep） |
| 架构 | x86_64 |
| 上一版本 | 0.149.0-rc2，SHA-256 `b2aa2890d5328e21cf3c606c0c77ca8b885d9514c28f131fdae108ad10f6d57d` |
| 候选 | 官方 Release `willdeep-linux-amd64.tar.gz`，tar SHA-256 `4d5ff60d…6e5fd` |
| 新二进制 | `willdeep 0.154.0-rc5`，SHA-256 `4f20f7c9991f24d0ffd61c4f6689c73de33776f156608e420baad204ab84c2eb` |
| 入口 | `/home/willdeep/.local/bin/willdeep`（willdeep:willdeep）；`/usr/local/bin/willdeep` 为指向它的符号链接 |
| 服务定义 | `willdeep-daemon.service` 未修改：User=willdeep，Restart=no，ExecStart 指向用户目录二进制 |

安装前任务均为终态：Completed 26、Cancelled 4、Failed 1；会话 Idle 5、Failed 1、Interrupted 1；人工等待为空。替换用户目录二进制后执行 `willdeep daemon upgrade`，Runtime 自交接完成，不需要 sudo 或重启 systemd。

## 安装后状态

| 组件 | 结果 |
| --- | --- |
| Runtime | `version=0.154.0-rc5`，PID 693979，`127.0.0.1:44101` |
| `willdeep-daemon.service` | active（oneshot 启动器已退出）、enabled |
| 符号链接 | `/usr/local/bin/willdeep -> /home/willdeep/.local/bin/willdeep`，版本 rc5 |
| web listener | 无，未启动 |

## 备份与回滚

备份：`/home/willdeep/.local/bin/willdeep.bak-0.149.0-rc2`，SHA-256 `b2aa2890…6d57d`。

回滚时恢复旧二进制并让 Runtime 再次交接：

```bash
ssh h2.niuwoai.com 'cp -p /home/willdeep/.local/bin/willdeep.bak-0.149.0-rc2 /home/willdeep/.local/bin/willdeep && willdeep daemon upgrade'
```

本轮安装成功，未执行回滚。

## 边界

- 未使用 sudo，未修改 systemd 定义、防火墙或 Tailscale。
- 未读取 provider 配置、凭据或任务正文；未使用真实模型请求。
- 升级后的 Runtime 只监听回环地址。
