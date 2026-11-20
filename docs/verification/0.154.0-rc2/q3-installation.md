# q3 升级至 0.154.0-rc2 的安装验收

2026-11-19 通过用户指定的 `q3-pub` SSH 入口完成。实施前重新查询 GitHub，最新公开发行仍是 [v0.154.0-rc2](https://github.com/niuwoai/willdeep-cli/releases/tag/v0.154.0-rc2)，对应发行提交 `0fb3a57bf1811c0bea767767bad0bb0befd31e5b`；[正式发行工作流](https://github.com/niuwoai/willdeep-cli/actions/runs/44448133912)为 success。本记录接续[正式发行验收](official-release.md)，此前发行阶段没有升级 q3，本次已完成。机器可读结果见 [q3-installation.json](q3-installation.json)。

**后续配置：** 本文保留安装完成时的历史快照。随后已按用户要求停止 root 独立 Runtime，并配置 `0.0.0.0:9847` → `127.0.0.1:19847`，通过 Tailscale 直连验收；当前状态和网络回滚方法见 [q3-network.md](q3-network.md)。本文下列 PID、控制地址、Web 监听和 root 运行状态均属于安装时点。随后已在保持网络布局和 root 停止的前提下试装 [rc3 HTTP 发送修复候选](../0.154.0-rc3/web-http-send.md)，当前应用版本与 PID 以该记录为准。

## 结果

| 项目 | 升级前 | 升级完成时 |
| --- | --- | --- |
| `/usr/local/bin/willdeep`，systemd 使用入口 | 0.153.0-rc5 | 0.154.0-rc2 |
| `/home/willdeep/.local/bin/willdeep`，willdeep 账号默认入口 | 0.149.0-rc2 | 0.154.0-rc2 |
| willdeep 账号 Runtime | PID 2329081，0.153.0-rc5 | PID 2980063，0.154.0-rc2 |
| root 账号独立 Runtime | PID 2287280，0.149.0-rc2 | 正常排空交接至 PID 2986001，0.154.0-rc2，控制地址 127.0.0.1:32995 |
| `willdeep-daemon.service` | active/running，enabled | active/running，enabled，MainPID 2980063，NRestarts 0 |
| Runtime 控制地址 | 127.0.0.1:42719 | 127.0.0.1:33341 |
| Runtime 托管 Web | 127.0.0.1:9847，0.153.0-rc5 | 原监听保留，0.154.0-rc2，enabled/running，error=null |
| 旧 `willdeep-w3-web.service` | disabled，但仍 auto-restart，备份时已重试 7589 次 | 停止重启循环，inactive/dead，保持 disabled |

原二进制 owner、group、0755 权限保留；三份 systemd 定义文件的摘要在升级前后完全一致。没有修改工作区、监听、代理、防火墙、Cloudflare 或 provider 配置，没有读取凭据正文。两套 Runtime 各自保留原服务归属与数据目录。

## 安装与交接

q3 实测为 Linux x86_64、glibc 2.41。将此前下载的官方 `willdeep-linux-amd64.tar.gz` 通过 SCP 传入全新独立目录，本机及 q3 两次核对 SHA-256：

```text
23ae891475dbfb810ba33c8dc9873fd0ab4033bee4d05c8a2dfd5edb06f9f361
```

该摘要与 GitHub asset digest 一致。检查归档唯一成员为普通可执行文件 `willdeep`，在另一个新目录解包后，原生执行报告 `willdeep 0.154.0-rc2`，`file` 确认 ELF x86-64。两个最终安装入口的 SHA-256 均为：

```text
d9ebfb925b5f5dd01dcc92197324a4c3d507e9510ac47c43536b58cc59fff450
```

`2026-11-19T05:16:16Z` 完成备份；`05:16:38Z` 原子替换两个入口；`05:17:09Z` 完成 Runtime 重启。

替换前及重启前分别查询任务、会话和人工 gate：6 个历史 Task 中 5 Completed、1 Cancelled；4 个 Session 中 3 Idle、1 Failed；pending 清单为空。这些终态及空闲状态在升级后保持相同，没有观察到新增 Interrupted 或取消任务。升级后通过独立 `approval.list` / `question.list` 查询，数量均为 0。

q3 使用 systemd `Type=forking` + `daemon start`。为保留 systemd 对新 MainPID 的托管，在确认空闲后执行正常 `systemctl restart willdeep-daemon.service`，未使用 `--force`。没有直接执行可能使新 Runtime 脱离该单元的 `daemon upgrade`。旧独立 Web 的重启循环在替换前停止，避免它在重启窗口争抢 9847 端口；没有删除旧服务或备份。

root 账号还有一套不属于 systemd 单元的独立 Runtime，最后核对时发现仍是 0.149.0-rc2。其 1 个 Task、1 个 Session 均为既有 Failed，pending 为空。在独立的 `root-runtime-before-0.154.0-rc2` 子目录备份运行中旧可执行文件，以及 8 份 Runtime 非凭据状态文件，逐份核对摘要；不备份或人工读取 Runtime token 元数据。随后在 root 身份执行 `/usr/local/bin/willdeep daemon upgrade --timeout 300`，于 `2026-11-19T05:24:40Z` 正常排空交接至 PID 2986001。任务和会话状态保持相同，未使用 force。通过后续独立 SSH 连接再次核对，Runtime status/capabilities 均为 rc2，运行中可执行文件摘要与官方候选二进制一致，systemd 托管的另一套 Runtime 和 Web 保持正常。

## 验收

- 两个绝对路径以及 `willdeep` 账号登录后的默认命令均报告 `0.154.0-rc2`。
- `runtime.status` 返回 PID 2980063、版本 rc2，与 systemd MainPID 一致。
- `runtime.capabilities` 返回 status=ok、协议 1.0、server_version rc2，154 个操作；CLI 内部处理认证，不人工读取 Runtime token。
- `webapp.status` 返回 enabled=true、running=true、listen=127.0.0.1:9847、error=null。
- Web `/health` HTTP 200，正文 `{"status":"ok","version":"0.154.0-rc2"}`，`X-App-Version: 0.154.0-rc2`。
- 首页及两个 JS、一个 CSS 均 HTTP 200，版本头 rc2；实际响应 SHA-256 与本地已验收发行构建逐字节一致。
- 本机通过 q3-pub SSH 转发真实打开远端 Web，首页渲染正常；点击“定时任务”后，列表、状态筛选和空状态正常渲染。浏览器没有控制台错误或失败网络请求。验收后恢复桌面视口并停止临时本地隧道。
- `rc5 → rc2` 没有新增数据库迁移要求；没有执行显式 migration up/down，也没有更改数据库 schema。Runtime 仍走原有启动恢复流程。

安装验收时 Web 继续只监听回环地址。当时验证的是 q3 本机接口和 q3-pub 的 SSH 转发访问链路，不能证明直接 `q3:9847`、局域网入口、公开反向代理或 Cloudflare 已可访问。升级前对 `192.168.0.108:9847` 的连接失败，与当时回环监听一致。后续外层监听及真实 Tailscale 直连结果另见[网络配置验收](q3-network.md)。

## 备份和具体回滚方法

远端备份目录：

```text
/var/backups/willdeep/20261119-0.153.0-rc5-before-0.154.0-rc2
```

目录包含两个原二进制、三份 systemd 定义及远端 `installation.json`。原二进制逐字节核对并执行 `--version` 验证；另外六份历史二进制备份继续保留，没有清理。

| 原二进制备份文件 | SHA-256 |
| --- | --- |
| `service-willdeep-0.153.0-rc5` | `83bdf973d45855408909cf45af13d22ffe63b334755207b4fcd6168e3dd12622` |
| `user-willdeep-0.149.0-rc2` | `b2aa2890d5328e21cf3c606c0c77ca8b885d9514c28f131fdae108ad10f6d57d` |

以下是二进制安装回滚步骤，**未实际执行回滚**。当前已有后续网络配置；若需要按下列步骤恢复旧二进制，应先按[网络回滚](q3-network.md#网络回滚)恢复回环 9847 并禁用新增 listener。root Runtime 保持停止，不恢复其运行状态。先登录 q3：

```bash
ssh q3-pub
```

以 root 执行以下只读命令，确认所有 Task 均已终止、Session 空闲，且没有待处理审批或问题；存在活动工作时应先等待完成，不能直接重启：

```bash
runuser -l willdeep -s /bin/sh -c '/usr/local/bin/willdeep daemon tasks; /usr/local/bin/willdeep daemon sessions; /usr/local/bin/willdeep daemon pending'
```

重新核对备份摘要和版本，须与上表一致：

```bash
sha256sum /var/backups/willdeep/20261119-0.153.0-rc5-before-0.154.0-rc2/service-willdeep-0.153.0-rc5 /var/backups/willdeep/20261119-0.153.0-rc5-before-0.154.0-rc2/user-willdeep-0.149.0-rc2
```

```bash
/var/backups/willdeep/20261119-0.153.0-rc5-before-0.154.0-rc2/service-willdeep-0.153.0-rc5 --version
```

```bash
/var/backups/willdeep/20261119-0.153.0-rc5-before-0.154.0-rc2/user-willdeep-0.149.0-rc2 --version
```

确认无活动工作及人工 gate 后，使用尚不存在的临时路径原子恢复服务入口：

```bash
test ! -e /usr/local/bin/willdeep.restore-before-rc2 && install -o willdeep -g willdeep -m 755 /var/backups/willdeep/20261119-0.153.0-rc5-before-0.154.0-rc2/service-willdeep-0.153.0-rc5 /usr/local/bin/willdeep.restore-before-rc2 && mv /usr/local/bin/willdeep.restore-before-rc2 /usr/local/bin/willdeep
```

恢复 willdeep 账号原命令入口：

```bash
test ! -e /home/willdeep/.local/bin/willdeep.restore-before-rc2 && install -o willdeep -g willdeep -m 755 /var/backups/willdeep/20261119-0.153.0-rc5-before-0.154.0-rc2/user-willdeep-0.149.0-rc2 /home/willdeep/.local/bin/willdeep.restore-before-rc2 && mv /home/willdeep/.local/bin/willdeep.restore-before-rc2 /home/willdeep/.local/bin/willdeep
```

随后让 systemd 启动恢复后的服务二进制：

```bash
systemctl restart willdeep-daemon.service
```

核对 Runtime 恢复至 rc5、MainPID 一致、Web 健康及版本头恢复，旧独立 Web 应保持停止且 disabled：

```bash
runuser -l willdeep -s /bin/sh -c '/usr/local/bin/willdeep daemon status'
```

```bash
systemctl show willdeep-daemon.service -p ActiveState -p SubState -p MainPID -p Result
```

```bash
curl --fail --silent --show-error --include http://127.0.0.1:9847/health
```

root 独立 Runtime 随后已按用户要求正常停止，当前回滚应继续保持 stopped。旧 root 二进制和 8 份状态文件快照仍位于备份子目录 `root-runtime-before-0.154.0-rc2/`；不主动启动旧实例，不默认回填 `runtime-state/`，以免覆盖升级后的新工作。若另行需要恢复数据，应先确认影响范围。

回滚不执行数据库 down，不重新启用旧独立 Web 服务。由于两个旧入口本来就不同版本，恢复入口后默认用户命令为 0.149.0-rc2、systemd Runtime 为 0.153.0-rc5，root Runtime 继续停止。

本次只安装现有官方发行包，未改变应用源码、版本号或已发布 tag；验收记录生成时尚未提交 Git。
