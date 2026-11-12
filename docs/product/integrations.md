# 插件与外部集成产品模块

> 范围：插件宿主、公网入口、分享、实例联邦、手机中继、出站通知、Mac 交接、终端集成及 SDK/CI。开发版本见[产品概览](../../PRODUCT_OVERVIEW.md)。宿主内部协作归[智能体模块](agents-collaboration.md)。

## 能力概要与用户入口

外部集成复用 Runtime 的会话、队列、审批和预算。Web 插件中心与公网入口页提供管理，CLI/TUI 提供 `plugin`、`fed`、`handoff`、`/mobile` 等入口；安装、批准插件、启动服务与发行状态分别核验，历史“内嵌版本”不表示此刻的最新发布组合。

## 已实现范围

### 插件宿主与外部 MCP

- Web 与 macOS 共享插件包，宿主页面桥提供文件、命令、代发网络、结构化存储、技能清单、生图、带插件媒体图片的模型请求、向主 Agent 递交文本与宿主事件。旧概览记录的桥版本为 2.6.0，详细兼容表以[插件系统](../PLUGINS.md)为准；视频附件与流式思考两项仅 macOS 提供。
- 未识别的清单能力显示“本宿主不支持”，不因词汇未知直接拒装；浏览器选择文件并上传替代原生文件框。图片参照与短剧背景音乐各有大小限制，按专题约束；包共享不代表审批或进程共享。
- 插件 MCP 网关允许外部客户端按需拉起插件，不必先开插件页；与 macOS 同时运行时只转给自己拉起的进程。插件进程可反向请求宿主出图、问模型，已启用插件 MCP 工具可进入聊天，仍受宿主权限保护。
- 插件中心编辑后按插件分别保存，显示未保存/保存中/已保存/失败；部分失败保留剩余修改重试，已保存密钥不回显。标题栏、图标与主题变量对齐，聊天只显示附件协议边界后的正文，选区菜单保持到执行或明确关闭。
- 收藏插件提供富文本备忘、图片、搜索与标签，数据留在当前 home 私有插件目录，明确启用仍需批准；Ruby 依赖与内嵌插件安装见[插件专题](../PLUGINS.md)。完整配置工作台已内置，不要求启用旧配置插件，见[Web 模块](web.md)。
- Web 对外 MCP 端点 `POST /mcp` 使用 Streamable HTTP，可在设置中创建 token 并取得外部安装指令，用于读取本机会话和任务；该入口是只读工具面，不能当作远程任意执行通道。MCP 客户端另归[工具与模型](tools-models.md)。

### Magpie 社区 Agent 插件

- 0.142.0-rc1 接入双端独立的只读 JSON 连接文件：Desktop 与 CLI 各自管理连接，共享 v1 Schema 和 fixture；CLI 以临时 `magpie-cli` Provider 解析，不把连接写进 TOML、桌面 Provider 库或钥匙串。配置只接受回环地址、完整模型 ID 和固定公开归因标记。
- 无显式选择的新会话采用当前连接；已有会话固定创建时的 Provider 与模型，外部连接切换不会改写。断开后显式引用集成 Provider 的会话返回不可用错误，不转发给其他供应商；旧的无固定选择会话恢复时跳过集成默认值。
- 实际请求禁用代理与重定向，`localhost` 固定到回环地址；原工具审批、沙箱和显式 Worker Profile 继续沿既有执行链。插件包、安装说明和社区收录状态见[实施台账](../MAGPIE_AGENT_PLAN.md)，需求与设计见[需求契约](../MAGPIE_AGENT_REQUIREMENTS.md)、[跨端设计](../MAGPIE_AGENT_DESIGN.md)，验收边界见[报告](../verification/0.142.0-rc1/magpie-agents/README.md)。

### 公网入口、分享与联邦

- Daemon 可在 Web 管理端口之外运行单独公网入口，承载静态站点、信息收集表单和 Webhook；表单/Webhook 投递普通消息到指定工作区会话。管理配置只允许回环 Web 监听读取，公网端口不能访问管理面。
- `public_url` 指定访客最终访问的域名，分享、站点和表单链接优先用它，留空才按监听拼；反代、端口与访问条件由[公网入口指南](../PUBLIC_INTAKE.md)维护。
- 表单设计器按字段生成页面、校验白名单；签名令牌 15 分钟有效且单次消费，拒绝伪造、过期与重放。令牌不识别提交者，也不阻止机器人重新取表单。
- `/share/<token>` 只投影会话正文，read 只读、chat 可继续发言；访客输入作为普通 Turn 按该会话当前授权档位执行，单条 8KiB、每 token 每分钟 20 条。分享不授予管理权限，也不允许公开访问完整子会话。
- 稳定实例 UUID 与可改名身份分开，改名不换 UUID；`/name`、`willdeep identity` 可查看/改名，中文/日文按终端列宽校验，重名可用短 ID 区分；实例名进入动态上下文但保留 WillDeep 产品身份。
- 实例联邦 P1 已实现两台公网可达宿主的 `fed invite/join` 配对及相互消息。入站进入 `[federation] workspace`，模型按策略回传，设连续自动回复上限、每分钟限流和 300 秒重放窗口；联系人密钥保存在独立 0600 文件，不进普通配置。
- 联邦 TUI `/fed`、CLI `willdeep fed`、Web 左侧导航的「联邦」入口与公网入口页标签共用同一实现。群聊由建群的一方星形转发，只有人的发言触发自动回复；Agent 的回复不再被回答，除非它用 `@名字` 点到某位成员，而这样的传递最多 3 次。连续自动回复到上限停止。端到端加密及中继仍属后续阶段，不能把[联邦文档](../FEDERATION.md)的设计章节当作已交付功能。

### 手机与跨端会话

- `/mobile`/二维码与 `daemon mobile enable` 开启 Runtime 托管中继；终端退出不关闭，重启按持久开关恢复。手机可看会话/历史、发送文本图片、在已登记工作区新建会话、停止 Turn、一次允许/拒绝审批及回答问题，动作绑定宿主已有操作。
- 手机运行配置协议已支持现有会话的 Provider、模型、四档审批、Skills 和已批准插件选择：先校验后持久化，回显真实生效状态，Provider/模型用于下一轮，审批更新真实句柄；只读工作区仍为上限。Rust 暂无同等专家机制，专家列表为空，不能远程批准/启用新插件。
- 手机、Web、Mac/Android 使用共享可编辑待发协议，保留完整正文、附件及宿主上下文；旧概览“手机不能改模型/档位”只作历史，不覆盖[手机配置协议](../MOBILE_RUN_CONTEXT.md)。
- 共享收件箱、完整审批与 Diff 审阅遵守目标归属、版本/快照校验，不可把摘要当完整审批。Rust 没有 macOS 同型“补丁提案”对象，跨设备 Patch 审核不因 Diff 阅读入口就算完成。
- macOS 交接通过 `willdeep/handoff/*` Git 分支，CLI handoff list/accept/watch 可导入安全副本并按 run 路径续做；双方各用自身凭据。Swift Project 元数据/历史兼容读取与工具结果关联保留，双向原地写入未开放。

### 通知、终端与 SDK

- `[notifications]` 与 macOS 共用配置语义，保留桌面声音偏好；任务完成/需要人处理时可向 Webhook、飞书机器人、SMTP/Resend 邮件多渠道旁路投递，逐渠道过滤和投递，一条失败不影响其他渠道或轮次结果。旧 `webhook_*` 兼容为一条渠道。
- Webhook 地址按普通偏好落盘；渠道凭据可引用环境变量，内联要求 Unix 配置权限 0600，`config show` 脱敏。报文和请求头仅由[出站通知](../NOTIFICATIONS.md)维护，不在产品台账复制 Schema。
- QTerm 接入按当前 TUI/无头会话及 Runtime 主任务报告工作、审批、回答与终态，只共享状态元数据。可选 Herdr 适配器从 Runtime Task 聚合状态并投影 Pane，不成为执行状态来源，也不泄露 Socket 路径。
- `willdeep-runtime-client` Rust SDK 与协议 crate 已具备 README、crate 文档和可运行示例；“发布就绪”不等于已经外发。CI 使用 `willdeep run` 的输出和退出码，见[CI 集成](../CI_INTEGRATION.md)。npm 组织包为 `@niuwoai/willdeep`，按对应 GitHub Release 下载并校验二进制；发行过程归安装/发布文档。

## 试点与当前限制

- 三集自动剧本为 0.119.0-rc3 本地开发试点，0.119.0-rc5 记录正常对话与页面共用 Runtime 写/审/改/导出。仅已明确启用且批准的工坊及宿主工作区可用；当时默认累计 50 万 token/64 次，追加额度需显式授权，不能将该记录当成新实验额度或最新发行包承诺。具体建议最多一次润色后重新双审，必改项按逐字证据关闭，试点不生成媒体。
- 短剧联合候选曾记录稳定素材最多四次图片请求、未知请求对账后明确授权恢复一次等边界；旧内嵌插件版本与未发布状态完整保留在[历史归档](history/overview-through-0.142.0-rc1.md)，实际 Runtime 工作流见[自动剧本专题](../SCREENPLAY_WORKFLOW.md)。
- 自定义专家圆桌经 `willdeep/roundtable/run` 顺序调用模型、保留发言和立场、主持收敛与终稿；当前返回完整结果，不提供 macOS 圆桌页面同型实时流式展示。
- 手机中继端到端加密未完成，服务端可见明文；相关设计在 Xedit 仓库的 `docs/MOBILE_GATEWAY_E2E_ENCRYPTION_DESIGN.md`。ACP/Codex App Server/Goose 接入、稳定共享会话 schema 后双向写入、签名 Computer Use Helper/AX/短租约仍是待办。
- 自动化联调不等于 Android 真机验收；安装、扫码、手机操作、Runtime 升级与发布分别记录，不因 CLI 更新就承诺现有 Runtime 自动重启或升级。

## 详细指南与证据

[插件](../PLUGINS.md)、[内置插件](../BUILTIN_PLUGINS.md)、[Magpie 社区 Agent](../MAGPIE_AGENT_PLAN.md)、[公网入口](../PUBLIC_INTAKE.md)、[联邦分期与实现差异](../FEDERATION.md)、[手机](../MOBILE.md)、[手机配置协议](../MOBILE_RUN_CONTEXT.md)、[通知](../NOTIFICATIONS.md)、[CLI 交接入口](../CLI_REFERENCE.md)、[Xedit 联动现状](../XEDIT_INTEROP_STATUS.md)、[Herdr 研究与集成](../HERDR_RESEARCH_AND_INTEGRATION.md)为详细事实来源。SDK 与控制接口见[Runtime 模块](runtime-sessions.md)，公共投影、凭据及权限见[安全与验证](security-verification.md)。
