# 0.154.0-rc5 q4-pub 安装验收

日期：2026-11-19T11:26Z 前后。结论：q4-pub 由 `0.153.0-rc5` 升级到 `0.154.0-rc5`，服务端使用官方 Linux amd64 产物，两份入口同步，Runtime rc5。机器可读记录见 [q4-installation.json](q4-installation.json)。

## 安装

| 项目 | 值 |
| --- | --- |
| 连接 | `q4-pub`（root） |
| 架构 | x86_64 |
| 上一版本 | 0.153.0-rc5，两份入口 SHA-256 均为 `83bdf973d45855408909cf45af13d22ffe63b334755207b4fcd6168e3dd12622` |
| 候选 | 官方 Release `willdeep-linux-amd64.tar.gz`，tar SHA-256 `4d5ff60d…6e5fd` |
| 新二进制 | `willdeep 0.154.0-rc5`，SHA-256 `4f20f7c9991f24d0ffd61c4f6689c73de33776f156608e420baad204ab84c2eb` |
| 入口 | `/home/willdeep/.local/bin/willdeep` 与 `/usr/local/bin/willdeep`，均为 root:root、0755 |
| 服务定义 | `willdeep-daemon.service` 未修改：User=willdeep，ExecStart 指向 `/home/willdeep/.local/bin/willdeep daemon start` |

安装前所有任务均为终态；会话列表里有一条 Interrupted，但没有活动 turn，人工等待为空。升级只替换两份二进制并重启 daemon，没有新增或修改 web listener。

## 安装后状态

| 组件 | 结果 |
| --- | --- |
| Runtime | `version=0.154.0-rc5`，PID 4147312，`127.0.0.1:41657` |
| `willdeep-daemon.service` | active（oneshot 启动器已退出）、enabled |
| 版本头 | Runtime API 返回 `x-willdeep-version: 0.154.0-rc5`；API 需要鉴权，未使用未授权健康检查冒充通过 |
| web listener | 无该 unit，未启动 |

## 备份与回滚

上一版本备份：

- `/home/willdeep/.local/bin/willdeep.bak-0.153.0-rc5`
- `/usr/local/bin/willdeep.bak-0.153.0-rc5`

两者 SHA-256 均为 `83bdf973…d12622`。回滚时把两份备份复制回原路径并重启服务：

```bash
ssh q4-pub 'cp -p /home/willdeep/.local/bin/willdeep.bak-0.153.0-rc5 /home/willdeep/.local/bin/willdeep && cp -p /usr/local/bin/willdeep.bak-0.153.0-rc5 /usr/local/bin/willdeep && systemctl restart willdeep-daemon'
```

本轮安装成功，未执行回滚。

## 边界

- 未读取 provider 配置、凭据或任务正文；未修改防火墙、Tailscale 或服务定义。
- q4 没有 web listener unit，本轮没有新增；Runtime 只监听回环。
- 未使用真实模型请求；升级以官方产物摘要和 Runtime 版本为准。
