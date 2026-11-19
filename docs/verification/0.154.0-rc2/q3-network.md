# q3 的 root Runtime 停止与 Tailscale Web 入口验收

2026-11-19，按用户明确要求，正常停止 root 账号独立 Runtime，保留 willdeep 账号的 systemd Runtime，并提供 `0.0.0.0:9847` 入口。应用版本保持 **0.154.0-rc2**。本记录接续[安装历史](q3-installation.md)，机器可读结果见 [q3-network.json](q3-network.json)。本记录保留 rc2 网络配置时点；随后 q3 试装了 [rc3 HTTP 发送修复候选](../0.154.0-rc3/web-http-send.md)，网络布局保持不变，当前版本和 PID 以该后续记录为准。

## 当前部署

访问地址：<http://100.85.114.57:9847/>，IP 来自 q3 的 `tailscale0`。

| 组件 | 用户 | 当前状态 | 监听或进程 |
| --- | --- | --- | --- |
| root 独立 Runtime | root | stopped，正常 `daemon stop`，未使用 force | 原 PID 2986001 已停止 |
| `willdeep-daemon.service` | willdeep | active/running、enabled、NRestarts=0 | 重启验收后 MainPID 3026741 |
| Runtime 托管 Web | willdeep | enabled/running、error=null | `127.0.0.1:19847` |
| `willdeep-web-listener.service` | willdeep | active/running、enabled、NRestarts=0 | `0.0.0.0:9847`，MainPID 3026752 |
| 旧 `willdeep-w3-web.service` | willdeep | inactive/dead、disabled | 未重新启用，未删除定义 |

数据流为：Tailscale 客户端 → q3 `0.0.0.0:9847` 的 TCP4 listener → `127.0.0.1:19847` 的 Runtime 托管 Web。Runtime 控制端口继续留在回环地址，未转发；重启后的随机控制端口没有另行记录，安装历史中的旧地址不能当作当前地址。

0.154.0-rc2 的 `daemon/web_app.rs`、CLI 和 TUI 托管 Web 入口均限制回环监听；直接要求托管 Web 使用 `0.0.0.0` 会被拒绝。因此使用 q3 已有的 socat 1.8.0.3 提供外部入口，托管 Web 保持在 Runtime 内运行。TCP 转发不修改 HTTP 正文，也不新增应用登录认证。按用户指定，外层绑定全部 IPv4 网卡，未限制为仅 Tailscale 网卡；没有修改防火墙或既有公开代理，也没有验证公网可达性。

只更改保存的 Web request 的 `listen`，其余工作区、配置引用、profile、语言等字段保持相同；没有打开引用的 provider 配置正文。root 的会话、任务和状态文件均保留；此次停止不构成账号级自动启动禁用，日后主动以 root 使用会启动 Runtime 的客户端仍可能创建新实例。

## systemd 单元与恢复

安装路径：`/etc/systemd/system/willdeep-web-listener.service`，root:root、0644。实际运行用户为 willdeep。最终文件 SHA-256：

```text
2f7f7c8221063189ee08d313f65d9151ecd3b890052eca0652b0a5f8a7f1e6a9
```

```ini
[Unit]
Description=Willdeep Web IPv4 listener on port 9847
Requires=willdeep-daemon.service
After=willdeep-daemon.service
PartOf=willdeep-daemon.service

[Service]
Type=simple
User=willdeep
Group=willdeep
ExecStart=/usr/bin/socat TCP4-LISTEN:9847,bind=0.0.0.0,reuseaddr,fork TCP4:127.0.0.1:19847
ExecStartPost=/usr/bin/curl --noproxy '*' --fail --silent --show-error --retry 10 --retry-connrefused --retry-delay 1 --connect-timeout 2 --max-time 3 --output /dev/null http://127.0.0.1:9847/health
Restart=on-failure
RestartSec=2
SuccessExitStatus=143
NoNewPrivileges=true
PrivateTmp=true
ProtectHome=true
ProtectSystem=strict
RestrictAddressFamilies=AF_INET AF_INET6

[Install]
WantedBy=multi-user.target
```

`systemd-analyze verify` 通过，已执行 `systemctl enable --now willdeep-web-listener.service`。`Requires`、`After` 关联 Runtime 服务，`PartOf` 让正常重启 Runtime 时同步重启 listener；`ExecStartPost` 等待 HTTP 就绪，避免把 systemd 刚启动进程等同于端口已可用。socat 的 SIGTERM 退出码 143 被视为正常停止。

配置于 `2026-11-19T06:15:57Z` 完成。再次确认所有 Task 终止、Session 空闲且 pending 为空后，执行正常 `systemctl restart willdeep-daemon.service`，于 `06:16:25Z` 完成恢复验收：Runtime 与 listener 都正常运行，内部 Web 的 19847 设置保留、外部 9847 健康检查通过，root Runtime 仍为 stopped。后续独立 SSH 连接再次核对进程、两个监听及 unit 摘要，结果一致。已设置开机启用；没有为了验收重启整台主机。

## 验收证据与范围

变更前和重启前均核对：6 个历史 Task 为 5 Completed、1 Cancelled，4 个 Session 为 3 Idle、1 Failed，pending 为空；没有使用强杀或 force。root 停止前也已确认其工作终止、无人工 gate。

内部 19847、外部 9847 及本机通过 Tailscale 直接访问 `/health` 均返回 HTTP 200、`X-App-Version: 0.154.0-rc2`，正文为 `{"status":"ok","version":"0.154.0-rc2"}`。

本机使用 `Net::HTTP.new('100.85.114.57', 9847, nil)`，明确禁用代理，没有 SSH 转发。本机路由走 Tailscale 的 `utun0`。重启后直接读取首页及全部入口资源：

| 路径 | HTTP | 字节数 | SHA-256 |
| --- | --- | --- | --- |
| `/` | 200 | 471 | `5caa092732e441f943ef0880f3f37e1f6544916fcfde822280d58a16f9c59021` |
| `/assets/index-D51g4cd4.js` | 200 | 970985 | `95e7311d84c40df621b7a2ecdc5e5324c79a763e32c2f0e6d296efdc6108ccaf` |
| `/assets/shareMessages-DIMqt2I4.js` | 200 | 358750 | `3ac5d9e3b68655d98b9eae8ef743f21c2ab64d6edc35da77ec2aeda9f12e4e49` |
| `/assets/index-C82JofDj.css` | 200 | 111809 | `6bbf559a8c749f7e36584bf9575a9498337bf460963eaf40297ef9d119228d6e` |

四项版本头均为 rc2，摘要与已验收发行构建相同。当前安装的 Browser preview 不支持 URL-only attach，无法直接附着到 Tailscale 地址；此次验证覆盖真实 HTTP、版本、资源完整性和服务重启恢复，未声称完成直接浏览器交互，也没有发送聊天或 LLM 请求。[安装历史](q3-installation.md)中的浏览器交互属于此前 SSH 转发验收，不作为本次浏览器直连证据。

## 首次失败与修复

`2026-11-19T06:14:32Z` 的首次配置，内部 Web 成功迁移到 19847，但 Type=simple listener 尚未完成 bind 就立即检查 9847，得到 Connection refused。脚本自动停止并禁用新 listener，按原 request 恢复 `127.0.0.1:9847`，恢复健康检查 HTTP 200，`rollback_errors=[]`。

保留首次失败报告，随后补上有界 `ExecStartPost` 就绪检查和 `SuccessExitStatus=143`，核对旧 unit 未被其他操作修改、恢复状态完整后重试。第二次配置通过，之后正常重启 Runtime 的恢复验收也通过。没有删除失败证据。

## 备份

远端目录：

```text
/var/backups/willdeep/20261119-web-listener-9847
```

| 文件 | 内容 |
| --- | --- |
| `webapp-service.json` | 原保存元数据，enabled=true、listen=127.0.0.1:9847，保留完整原 request |
| `network-configuration-attempt-1.json` | 首次失败及自动恢复记录 |
| `network-configuration.json` | 成功配置和 daemon 重启恢复记录 |

原元数据 SHA-256：

```text
bc898684928fdca0984fbc8db6c6796e25a3db148bc344c81324895bc5b8d516
```

远端实施脚本为 `/root/willdeep-network-0.154.0-rc2-20261119/q3-configure-web-listener.rb`，只提供 `configure` 和 `restart-check` 两种模式；没有 `rollback` 子命令。此前的安装包、二进制和状态备份继续保留，见[安装历史](q3-installation.md#备份和具体回滚方法)。

## 网络回滚

以下步骤恢复本次网络改动前的回环 Web，保持 rc2 应用、root Runtime 停止及旧独立 Web 禁用。最终成功配置**没有执行回滚**；首次失败的自动恢复已实际通过。手动回滚命令已审阅，未为测试再次中断当前可用入口。

先登录 q3：

```bash
ssh q3-pub
```

以 root 查询 willdeep 账号 Task、Session 和 pending。存在活动工作或人工 gate 时先等待，不能直接停止 Web：

```bash
runuser -l willdeep -s /bin/sh -c '/usr/local/bin/willdeep daemon tasks; /usr/local/bin/willdeep daemon sessions; /usr/local/bin/willdeep daemon pending'
```

核对备份摘要须与上文一致：

```bash
sha256sum /var/backups/willdeep/20261119-web-listener-9847/webapp-service.json
```

停止并禁用外层 listener，释放 9847：

```bash
systemctl disable --now willdeep-web-listener.service
```

以 willdeep 身份停止内部 Web，确认响应 `status=ok` 后再继续：

```bash
runuser -l willdeep -s /bin/sh -c '/usr/local/bin/willdeep api webapp.stop --ndjson'
```

从原启动元数据提取完整 request，经标准输入交给同一账号的 Runtime；此命令只读取启动元数据，不读取它引用的 provider 配置，也不生成或覆盖文件。确认响应 `status=ok`、listen=127.0.0.1:9847：

```bash
ruby -rjson -e 'puts JSON.parse(File.read(ARGV.fetch(0))).fetch("request").to_json' /var/backups/willdeep/20261119-web-listener-9847/webapp-service.json | runuser -l willdeep -s /bin/sh -c '/usr/local/bin/willdeep api webapp.start --params-file - --ndjson'
```

核对回环健康及版本头 rc2：

```bash
curl --noproxy '*' --fail --silent --show-error --include http://127.0.0.1:9847/health
```

检查 root Runtime 仍 stopped，旧 Web 与 listener 均 inactive/disabled，9847 只剩回环监听：

```bash
/usr/local/bin/willdeep daemon status
```

```bash
systemctl show willdeep-w3-web.service willdeep-web-listener.service -p Id -p ActiveState -p SubState -p UnitFileState
```

```bash
ss -lntp 'sport = :9847 or sport = :19847'
```

回滚不启动 root Runtime，不重新启用旧独立 Web，不恢复安装前旧二进制，不执行数据库 down，也不删除服务定义或备份。

本次没有数据库迁移、应用源码修改、版本变更、tag 变更或 Git 提交。部署拓扑已同步至[架构文档](../../ARCHITECTURE.md#安全边界)。
