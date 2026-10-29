# 出站通知渠道

`[notifications]` 负责**事后的礼貌通知**：任务完成、或者 Agent 停下来等人处理时，把一句话发到外面。它和 [生命周期 Hook](HOOKS.md) 是两回事——hook 跑在关键路径上、能拦下动作；通知是旁路，可以慢、可以失败，绝不影响轮次结果。

代码在 `crates/willdeep-cli/src/notify/`：

| 文件 | 职责 |
|---|---|
| `notify.rs` | 事件产生、去重、每条渠道一个 detached 任务、错误汇总 |
| `notify/channels.rs` | 配置 → 渠道，投递分发，旧的 `webhook_*` 兼容 |
| `notify/lark.rs` | 飞书自定义机器人报文与签名 |
| `notify/email.rs` | SMTP（lettre）与 Resend HTTP API 两种发信后端 |
| `notify/render.rs` | 飞书正文与邮件主题的多语言文案 |

## 兼容：旧的 `webhook_*` 仍然有效

`[notifications]` 这一段与 WillDeep.app 共用。原来的四个标量键没有变化：

```toml
[notifications]
webhook_enabled = true
webhook_url = "http://127.0.0.1:8787/willdeep"
webhook_on_task_completed = true
webhook_on_attention_required = true
```

它等价于一条名叫 `webhook` 的渠道，继续发 `willdeep.webhook.v1` 报文。新的
`[[notifications.channels]]` 是**追加**，不需要先删掉旧配置；两边可以同时存在。
`[notifications]` 里的未知键继续被忽略，桌面端加字段不会让旧版 CLI 拒绝启动。

## 渠道配置

```toml
# 1. 通用 Webhook，可带 HMAC 签名
[[notifications.channels]]
kind = "webhook"
name = "dashboard"
url = "https://example.com/willdeep"
secret_env = "WILLDEEP_WEBHOOK_SECRET"   # 可选；也可直接写 secret = "…"
events = ["task_completed", "attention_required"]

# 2. 飞书自定义机器人
[[notifications.channels]]
kind = "lark"
name = "team-bot"
webhook_url = "https://open.feishu.cn/open-apis/bot/v2/hook/xxxx"
secret_env = "WILLDEEP_LARK_SECRET"      # 机器人开了「签名校验」才需要；也可直接写 secret

# 3. 邮件：SMTP
[[notifications.channels]]
kind = "email"
name = "ops-mail"
from = "WillDeep <bot@example.com>"
to = ["ops@example.com"]
smtp = { host = "smtp.example.com", port = 587, tls = "starttls", username = "bot@example.com", password_env = "WILLDEEP_SMTP_PASSWORD" }

# 4. 邮件：Resend HTTP API
[[notifications.channels]]
kind = "email"
name = "ops-resend"
from = "bot@example.com"
to = ["ops@example.com"]
resend = { api_key_env = "RESEND_API_KEY" }
```

通用字段：

| 字段 | 必填 | 说明 |
|---|---|---|
| `kind` | 是 | `webhook` / `lark` / `email` |
| `name` | 否 | 诊断输出里的名字；不写按 `kind-序号`（如 `lark-2`）。同名字不能重复 |
| `events` | 否 | 订阅的事件；缺省两种都发。写空数组会被拒绝——不想收就删掉这条渠道 |

`events` 只认两个值：`task_completed`（一轮完成）和 `attention_required`
（等待批准、等待回答、被阻塞）。重复写同一个事件会被拒绝。

### webhook

| 字段 | 说明 |
|---|---|
| `url` | 必须是 http/https |
| `secret` / `secret_env` | 可选，二选一：签名密钥原文，或它的环境变量名。设置后每个请求带 `X-Willdeep-Signature: sha256=<hex>`，签名对象是**原始请求体**，与 [`[intake]` 验签](PUBLIC_INTAKE.md)同一套算法 |

报文与桌面端一致，见 [共享 Schema](../PRODUCT_OVERVIEW.md)。请求头包括
`X-App-Version`、`X-Agent-Source`、`X-Agent-Executor`、`X-Webhook-Schema`、
`X-Agent-Event`。

### lark（飞书自定义机器人）

| 字段 | 说明 |
|---|---|
| `webhook_url` | 群机器人地址，必须是 http/https |
| `secret` / `secret_env` | 可选，二选一：机器人开启「签名校验」后的密钥原文，或它的环境变量名 |

报文是 `{"msg_type":"text","content":{"text":"…"}}`；配置了密钥时额外带
`timestamp` 与 `sign`：

```
sign = base64(HMAC-SHA256(key = "{timestamp}\n{secret}", data = ""))
```

**成功不等于 HTTP 200**：飞书的业务错误同样返回 200，只有响应体
`code == 0` 才算送达；`code` 为其它值时按 `code` + `msg` 记错误。正文上限
10 KB（飞书自己的上限是 20 KB，留了一半余量）。自定义机器人的频率限制约
100 条/分钟、5 条/秒；本仓库不额外限流，事件本来是每轮一两条，不会触发。

飞书只收人读得懂的文字，不解析 `willdeep.webhook.v1` 报文。正文按 `[agent]
language` 渲染：标题、会话、会话 ID、状态、任务、触发原因、摘要。

### email

| 字段 | 说明 |
|---|---|
| `from` | 发件人，支持 `名字 <地址>` |
| `to` | 至少一个收件人，数组 |
| `smtp` | SMTP 后端，见下 |
| `resend` | Resend 后端，见下 |

`smtp` 与 `resend` **必须二选一**；两个都写或都不写都会被配置校验拒绝。

SMTP：

| 字段 | 默认 | 说明 |
|---|---|---|
| `host` | 必填 | 不能带空白或控制字符 |
| `port` | 按 `tls` 推断 | 465 / 587 / 25 |
| `tls` | `implicit` | `implicit`（连上即 TLS）、`starttls`（先明文再升级）、`none` |
| `username` | 无 | 有它就必须有密码，反之亦然 |
| `password` / `password_env` | 无 | 密码或授权码原文，或它的环境变量名，二选一 |

`tls = "none"` 且带凭据时，只允许回环主机（`localhost`、`127.0.0.1`、`::1`）：
明文连接上传密码等于把密码交给整条链路，本机中继（MailHog、本机 Postfix）
是唯一的合理例外。SMTP 走 lettre 的 rustls 栈，不引入 OpenSSL。

Resend：

| 字段 | 默认 | 说明 |
|---|---|---|
| `api_key` / `api_key_env` | 必填其一 | API Key 原文，或它的环境变量名，二选一 |
| `api_base` | `https://api.resend.com` | 自定义网关或测试服务器 |

`POST {api_base}/emails`，Bearer 鉴权，请求体
`{"from","to","subject","text"}`。**只有响应里带 `id` 才算成功**——202 只代表
排队。错误体按 `{statusCode,name,message}` 解析，只把 `name`/`message` 写进
日志。Resend 要求发件域名已在其后台验证，否则会返回 `validation_error`。

每次投递带 `Idempotency-Key: willdeep-{session_id}-{event}-{occurred_at}`
（没有会话 ID 时省略中间段），同一事件重投时 Resend 会去重。SMTP 没有对应
机制，重投就是多发一封。

邮件主题是 `[WillDeep] <事件> · <会话标题>`，超过 120 字符截断；会话标题来自
模型，拼进主题前会把换行压成空格，防止 header 注入。正文是纯文本。

## 密钥

每个密钥位置都有两种写法，**二选一**：

| 内联 | 环境变量 | 用于 |
|---|---|---|
| `secret` | `secret_env` | webhook、lark |
| `password` | `password_env` | smtp |
| `api_key` | `api_key_env` | resend |

- 两个都写会被配置校验拒绝；内联值是空白也会被拒绝。
- 内联密钥会让整份配置文件成为敏感文件：Unix 上要求权限 `0600`
  （`chmod 600 ~/.willdeep/config.toml`），否则校验直接报错；Windows 不检查
  文件权限，请自行保证文件不可被其他用户读取。
- `willdeep config show` 会把内联密钥替换成 `[REDACTED]`；环境变量名不是秘密，
  原样显示。
- 渠道在构造时解析密钥：环境变量不存在或为空，只跳过这一条渠道，并记一条
  「哪个变量没设」的错误；其余渠道照常投递，配置本身仍然合法，`willdeep run`
  不会因此拒绝启动。
- 密钥包装在 `Secret` 里，`Debug` 输出 `<redacted>`；reqwest 的错误串会剥掉
  完整 URL（webhook 地址本身可能就是凭据）；诊断输出从不打印端点。

## 投递语义

- 一次事件先算一份 payload，再按每条渠道的 `events` 过滤。
- **每条渠道一个 detached 任务**：一条 SMTP 卡住不会拖慢飞书，也不会把轮次
  拖失败。错误按渠道名汇总，最多保留 8 条。
- 请求上限 15 秒、连接上限 3 秒；进程退出前的 `flush` 最多等 6 秒。TUI 退出
  不等待投递，headless 路径（`willdeep run`）会等。
- 失败只出现在两个地方：headless 的 stderr，或 TUI 的「通知渠道」notice。

## 诊断

`willdeep doctor` 的 `notifications` 项列出能用的渠道（不含 URL 与密钥）：

- 全部渠道正常：`channels: webhook, lark:team-bot, email:ops-mail (smtp)`；
- 有渠道被跳过：再给一条 `skipped channel: …` 的警告，写明缺哪个变量；
- 一条都没配成：判为失败，并把原因带出来。

Web 设置页只编辑 `[notifications]` 里的标量字段（提示音、旧 webhook），不会
重写 `[[notifications.channels]]`；渠道直接改 `config.toml`。桌面端保存通知
设置时整段替换 `[notifications]`，替换范围在第一个 `[[…]]` 表头前结束，因此
渠道表不会被删掉。

## 扩展一个后端

发信后端与渠道是两层：

1. 新的发信方式（Mailgun API、SES API…）：在 `notify/email.rs` 给
   `EmailTransport` 加一个变体，实现 `send`；配置里加一个 `Option<…Settings>`
   并在校验里加「三选一」。渠道层不需要动。
2. 新的通知目标（Slack、Discord…）：在 `notify/channels.rs` 给 `Channel` 加
   变体，在 `config.rs` 给 `NotificationChannel` 加一个 tag，写自己的
   `deliver` 与 `describe`。

新增事件类型要同时改：`NotificationEvent`、`NotificationEventKind`、
`WebhookPayload` 的事件映射、`render.rs` 的三语文案，以及配置里的 `events`
取值。
