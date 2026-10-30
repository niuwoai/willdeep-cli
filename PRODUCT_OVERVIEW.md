# Product Overview

> 最后更新：2026-10-30 | 当前版本：v0.134.0-rc5

0.134.0-rc5 修复 `@niuwoai/willdeep` 的 npm 页面 README 为空。0.134.0-rc4 把 README 放进了包，但 npm 10 发给 registry 的元数据重读自磁盘上的 `package.json`，不带打包时写入的 readme，页面仍显示没有 README。发布脚本现在把 README 写进 `package.json` 再发布；安装方式不变。

0.134.0-rc4 给 `@niuwoai/willdeep` 补上 npm 页面的 README。包里此前没有这个文件，npmjs 上每个版本都显示没有 README；安装方式不变，仍按包版本下载对应的 GitHub Release。

0.134.0-rc3 把子代理的最大回合数上限从 32 提到 1000，与主会话一致；配置页在写入前校验整数区间，不再出现存得下、运行时才报错的情况。

0.134.0-rc2 与 0.134.0-rc1 功能相同，只把会话选择器里放在测试模块后面的状态文案函数挪回前面，clippy 的 `items_after_test_module` 因此通过。

0.134.0-rc1 给分凉出来的会话记下来源：Web 与 TUI 的会话列表把它缩进在来源正下方，对话标题下显示「分凉自「来源」· 多久前」。来源不在当前列表里时，它仍作为普通会话显示。

0.133.0-rc7 修复上下文压缩丢掉思维链回传信号：压缩替换掉最后一段非空思维链后，摘要和归档引用带一个空标记，下一轮仍给每条 assistant 消息补空 `reasoning_text`，不再被 thinking 模式整条拒绝（HTTP 400）。标记不冒充思维链原文；从没出现过思维链的会话不发这个项。

0.133.0-rc6 与 0.133.0-rc5 功能相同，只修正一处 clippy（`obfuscated_if_else`）。rc5 的 tag 已指向 CI 不通过的提交，按发版规则不移动，递增 rc 重发。

0.133.0-rc5 修复 Responses 接口 thinking 模式的工具续轮：历史里出现过思维链时，每条 assistant 消息都回传 `reasoning_text`，直给步骤和压缩替换掉的摘要、归档引用补空串，不再整条请求 400。没见过思维链的端点不发这个项。

0.133.0-rc4 修正 TUI 简洁模式渲染改动的 rustfmt 格式，发布流水线的格式检查不再因此失败。功能与 0.133.0-rc3 相同。

0.133.0-rc3 调整 TUI 简洁模式的过程显示：干活时聊天区只留计划卡片和一行会替换的状态（最近一段中途文字、旋转符、已运行时长），轮次收尾才落成正文；详细模式仍逐段显示。连续的思考记录两种模式都只显示最后一条，点开过的留在原处，原始记录不删除。

0.133.0-rc2 降低 Web 会话事件轮询的开销：`/api/runtime/events` 的会话归属校验按会话文件指纹缓存，文件没变就不再整读会话；执行指纹改为按需计算，只读加载不算它，只有改执行状态再写回的路径才在改动前算一次。

0.133.0-rc1 把出站通知从单条 webhook 扩展为多渠道：`[notifications]` 下可追加飞书自定义机器人与邮件（SMTP 或 Resend）渠道，每条渠道独立过滤事件、独立投递，一条失败不影响其余，也不影响轮次结果；旧的 `webhook_*` 配置继续有效，等价于一条名为 `webhook` 的渠道。邮件后端可插拔；密码与 API Key 可以内联写在配置里，也可以只引用环境变量名，内联时要求 Unix 配置文件权限 `0600`，`willdeep config show` 统一脱敏。本版同时包含原计划随 0.132.0-rc2 发布的两处修复：TUI 最新轮次里夹着思考记录的工具调用重新按段折叠，Web 本机回环监听时默认列出 Runtime 注册表里全部仍存在的工作区。

0.133.0-rc1 同时支持为每个 Agent 配置本机上下文窗口与最大轮次：Web 模型绑定面板和 `agents bind` 均可管理，Hermes 导入映射 `model.context_length` 与 `agent.max_turns`；明确填写的预算在 Worker 档位选择后应用，配置只保存在本机安装记录中，不进入可分享的定义包。

0.132.0-rc1 为执行计划卡的状态增加独立图标：待开始、进行中、已完成、已跳过、失败、阻塞各有对应图形，等待依赖的步骤使用时钟图标；进行中的图标持续旋转，并遵循系统的减少动态效果设置。TUI 在输出流中补建缺失的计划状态卡：中途接入 Runtime 会话或漏掉计划事件时，同步到的持久化计划也会显示为一张卡片，后续同步原位更新，不再需要等新事件才出现。

0.131.0-rc2 修复 Web 工具调用展开后的行重叠：展开步骤改为标签与耗时一行、命令详情一行，列表超过高度时内部滚动，不再把多行文字压进相邻步骤。会话记录与工具执行不变。

0.131.0-rc1 增加单条用户长消息折叠：Web 按实际高度预览约 12 行，TUI 按终端换行预览 8 行，均可点击展开与收起。Web 支持键盘操作和窄屏重排；TUI 普通、简洁模式及搜索跳转均适配。折叠仅影响客户端显示，发送、会话存储和完整原文保持不变。

Web 待回答问题统一在聊天区操作：每个选项占一整行，可直接点击回答，或在「其他回答」输入框提交；侧栏仅显示提醒并定位到对应卡片，避免重复表单与窄栏溢出。多选问题仍需选好后提交。

0.130.0-rc1 增加 Web Hermes/Multica 导入转换及模型绑定确认。Hermes 支持 config.yaml 与 SOUL.md，Multica 支持 Agent API JSON。未映射字段显式列出，默认只读职责；技能、上下文预算、轮次和外部 Runtime 配置需另行配置。历史轮次默认隐藏思考行、折叠工具调用，最新轮次保留过程。

0.129.0-rc6 修复 TUI 空输入框的 ↑ 行为：紧贴输入框显示当前会话最近 100 条去重的历史输入，支持上下选择和 Enter/Tab 回填，回填后可编辑并手动发送；Esc 关闭，已有草稿继续使用原光标导航。

0.129.0-rc5 修复配置页错误提示：保留服务错误与 HTTP 状态，识别代理登录页和无效响应，便于区分配置服务不可用、认证及代理问题。

0.129.0-rc4 修复计划卡片状态滞后：完整步骤对应的后台命令通过 `plan_step_id` 绑定，或用 `update_plan` 的 `job_id` 关联已有作业；运行时按实际退出状态独立更新步骤并实时推送 TUI/Web。单个任务完成无需等待模型或其他任务，失败和未知状态不会放行依赖。普通命令和较大步骤中的中间命令不自动视为整个步骤完成。

0.129.0-rc3 修复 Responses thinking 工具续轮：保存并回传 DeepSeek 原始 `reasoning_text`，覆盖流式、非流式与中断响应；summary 不作为原始推理回放，既有会话已丢失的推理不会凭空恢复。


0.129.0-rc1 增加会话计划的依赖关系：普通会话和目标模式都能通过 `update_plan` 声明步骤 ID 与 `depends_on`，执行器校验依赖并返回可启动步骤，Agent 使用已有后台任务和只读工具并行能力推进独立工作。共享文件、版本号、Git 和部署资源需通过依赖串行安排，原审批及预算限制保持生效。计划卡片展示多项进行中及等待原因；步骤状态按工具成功回执更新并持久保存，恢复时先核对已有任务，计划状态本身不作为完成证据。回复风格改为默认简洁：长任务只更新计划卡片，`/verbose` 切回每阶段一句正文，`/short` 切回简洁。

0.128.0-rc1 给长会话加了归档渲染窗口：TUI 和 Web 都只排版最近一段，更早的轮次折在聊天区顶部，滚到顶再放回，记录本身不截断。TUI 目标模式完成时显示状态行，`<goal-status>` 标签不再出现在聊天里；`ask_user` 问题被撤回时，问题和原选项会留在记录中。

0.127.0-rc2 的 TUI 指令菜单里，不需要参数的指令回车直接生效，聊天记录也不再回显 `/goal` 拼在前面的目标包装。

0.127.0-rc1 在 Web 服务上增加 MCP 端点（POST /mcp，Streamable HTTP）。在 Web 设置里创建 token 并复制安装指令，另一台机器上的 Claude Code 或 ChatGPT 就能读取这台 willdeep 的会话和任务；这一版只有只读工具。

0.126.0-rc4 把 npm 包改到组织作用域 `@niuwoai/willdeep`，安装命令变为 `npm install -g @niuwoai/willdeep@rc`，装好后的命令名仍是 `willdeep`。

0.126.0-rc3 把 `web.rs` 的测试模块拆到 `web/tests.rs`，让源码行数回到 3000 行上限以内。

0.126.0-rc2 修正发版门禁：`cargo fmt --check` 不再因 `/short` 的三处换行报错。

0.126.0-rc1 让空闲时也能选择先入队、以后再发：TUI 的 Cmd+Enter 与 Web 的 ⌘Enter 把当前输入写入 Runtime 队列并暂缓，会话空闲时不会被立刻发出，在队列里选择立即发送后才参与调度。

0.125.0-rc1 的配置页增加全局「默认模型」，选的是默认提供商实际使用的模型，保存时写回那个提供商的 `model`。上下文窗口、最大输出 Token 和 Token 预算可以一键填入 32K、64K、128K、256K、1M，并认出提供商的上下文窗口和子代理的回合上限。

0.124.0-rc1 增加 `/short`：会话级的简洁回复开关。开启后长任务用计划卡片汇报进度，聊天只留开工一句和最后结果；默认的每阶段一句正文进度保持不变。

0.123.0-rc8 修正公网入口的发版门禁：clippy 在 `-D warnings` 下不再报错。

0.123.0-rc7 增加公网入口：daemon 在 Web 的 9847 之外再监听一个端口（默认 9857），前面接 sslcat 或 nginx-proxy。一个端口同时发布静态站点、信息收集表单和 webhook，表单提交与 webhook 作为消息进入指定工作区的会话。

0.123.0-rc6 修正 Web 审批按钮：允许一次是实心主按钮，拒绝是描边次按钮；代码块右上角可复制，并提示复制成功或失败。

0.123.0-rc5 修正 npm 发布：GitHub Release 跳到带默认端口 443 的地址时不再被当成不受信任的重定向。

0.123.0-rc4 修正发版检查：工具回放摘要保留完整命令，rustfmt 通过。

0.123.0-rc3 让 Web 思考过程与 TUI 一样只显示耗时、点击展开；工具命令摘要不再只留前三个词；托管站点可浏览 some.im 精选目录。

0.123.0-rc2 增加 npm 发行：`npm install -g willdeep@rc` 下载并校验对应 GitHub Release 的平台二进制。

0.123.0-rc1 联合发布候选集成短剧工坊 0.53.0-rc27：图片制作按稳定素材累计最多四次请求，未知图片请求经对账与明确授权可恢复一次，恢复入口原样转发逐素材重试策略。Web 智能体库默认展示精选目录，支持搜索、刷新、安装状态与创建副本进入 RSI。

0.122.0-rc3 修复命令关键词误判：危险词仅在真实命令位置参与判断，普通 grep/echo 参数保持为数据；wrapper 和 find 执行载荷仍检查真实危险命令。

0.122.0-rc3 联合发布候选集成短剧工坊 0.53.0-rc9：Runtime 持久管理完整制作、预算、暂停恢复及交付文件；与完整配置工作台共存。Magpie 为本地网关后的远程推理，账本不再计入本地模型。

当前开发版本：0.129.0-rc3。Web 主包内置 Provider 快速设置与完整配置工作台，支持配置快照/schema、变更预览与应用、配置校验、备份恢复、模板生成、Provider 模型列表，以及 Provider、MCP 服务和子代理集合管理；无需启用或安装 willdeep-config 插件。配置工作台通过 loopback Web API 调用随 CLI 内嵌的配置服务，API Key 仍按原有规则脱敏。TUI 推理过程折叠为可点击的 Thought 耗时摘要，展开后查看完整内容。Runtime 是已提交队列的权威来源，Android、Web 与 Runtime TUI 同步展示和修改同一队列；输入框草稿仍属于客户端。Web 刷新和 TUI 退出不会移除 Runtime 已接受的消息。

人工请求归属会话，同一会话可跨端恢复，其他会话不能代答；Web 请求携带选中的会话身份，切换时忽略旧快照。重连回放中的缺失工具结果只表示尚未记录结果，不推断为失败。

人工等待可让出给独立消息：共享配置 `[agent] human_wait_yield_seconds = 300`（0 关闭自动让出）。调度器采用无工具的保守模型分类，依赖或不确定时保留队列，也可由用户明确标记独立任务。原确认不产生默认结果，旧答案在安全边界恢复，独立执行历史与原模型上下文分别保存。

正常对话会优先将完整三集剧本委托路由到已批准工坊的 Runtime 工具，保留宿主审批及预算保护。正常对话与工坊「自动剧本」页面已接入同一 Runtime 工作流：一句选题启动，显示写作、审核、修订和用量，支持带校验的定稿下载。控制桥只允许已批准工坊及宿主工作区；默认累计 50 万 token /64 次，追加额度必须显式授权。发布候选 0.119.0-rc5；原 Runtime 不随 CLI 发布重启或升级。

分集审稿的具体建议最多触发一次润色，润色后重新双审，必改项仍按逐字证据关闭。

三集自动剧本试点（0.119.0-rc3，本地开发）：`willdeep screenplay start/status/list/cancel/resume` 经持久 Runtime 组织插件写审改及导出；模型调用预算、任务身份、插件回执跨客户端断开保留。需要显式启用并批准受测插件；不升级已运行 Runtime，不生成媒体。

QTerm 原生接入按当前 TUI/无头会话及 Runtime 主任务报告工作、审批、回答与终态；只共享状态元数据，既有 Runtime 不随 CLI 更新重启。

手机网关与 Mac / Android 共用待发队列协议：返回完整可编辑正文，支持原位编辑、删除、清空与立即发送；修改校验原文并保留附件及宿主上下文。

命令行只读查询只连接现有 Runtime，包括 `daemon tasks`、`daemon sessions`、`session list`、定时任务与通用 API 查询；停止或状态失效时返回错误，不自动启动、重启或清理状态。TUI 每次检测到 Runtime 不可用时自动尝试启动一次；`/version` 保留连接或快照错误及恢复命令。手动启动可运行 `willdeep daemon start`；实际提交和修改操作继续按原规则启动 Runtime。

TUI 用户和助手消息末尾显示紧凑相对时间（刚刚、1分前、3天前），时间独立保存在本机，不写入模型上下文；无时间的旧消息保持原样。默认 Runtime TUI 与 Web 在忙碌时将输入和附件持久保存到 Runtime 队列，由 Runtime 统一调度；空闲时也可以主动入队并暂缓，直到在队列里选择立即发送。人工等待期间不把新输入当成原问题的回答。显式本地执行保留原有本地插话与队列语义。

命令或附件消息成功启动下一轮时，用户原文移动到上一轮结束分隔线之后、新回复之前；重复消息各自回显，失败时保留待发状态。

待发队列的底部操作使用带背景色的可点击按钮；立即发送、删除、保存与取消编辑的成功反馈在 3 秒后消失，失败信息持续保留。管理 Runtime 队列不会暂停宿主调度；已经开始的消息拒绝过期编辑。显式本地队列保留原有暂停语义。

审批弹窗标题按完整多语言文本居中显示，包含待审批队列数量提示。

完全访问模式允许明确的项目子路径和临时目录子路径清理（`rm` / `rmdir`），无需逐次审批。路径会检查符号链接和目录切换；项目/临时根目录、`.git`、越界路径及动态 Shell 表达式继续询问，其他审批模式不变。

本地 Provider：启动、打开 `/model` 或 F5 刷新时自动探测正在运行的 Ollama（默认 `127.0.0.1:11434`）与 Magpie（默认 `127.0.0.1:3425`，支持保存的端口）。尊重 `OLLAMA_HOST` / `MAGPIE_ADDR` 的本地地址，读取模型列表后自动保存 Provider；已配置的地址去重，不覆盖默认 Provider。仅访问回环地址，绕过代理，不携带云端凭据，不启动服务或下载模型。`WILLDEEP_LOCAL_DISCOVERY=0` 可关闭发现。

Web 一级导航在所有页面保持统一尺寸与样式，条目独立滚动，插件中心固定可达，智能体与 RSI 不再切换为另一套宽侧栏。

Web 与会话协作：在 v0.109.0-rc2 基础上增加项目范围的持久收件箱、关联回复、独立已读和模型消费回执。TUI 提供 `/peers`，Web 和 Android 使用同一协调收件箱；手机完整审批与 diff 审阅保持版本与归属校验。

Web 聊天使用宽正文画布，协作与定时任务从标题栏按需展开，收起保留草稿，兼顾窄屏和深浅主题。宿主路由仅在展示投影隐藏，原始模型上下文保留。

终端 UI 偏好：`[ui].mode` 可选 `full`（完全 UI，未配置时默认）或 `compact`（简洁 UI）。`/uistyle` 可在当前 TUI 会话中切换，`full` / `compact` 显式指定；`status` / `no-status` 显示或隐藏简洁 UI 底部状态栏，不改变当前布局。该命令不改配置文件；`willdeep-config` 保存的启动默认值仍在重启后生效。简洁 UI 隐藏主聊天、输入框的四边框、活动区与权限标签，只用一条标准细线分开正文与输入，整个底部背景铺满终端右下边缘；输入区使用跟随 QTerm 主题的 ANSI 黑色背景，青色输入文字与灰色建议文字保持可读。空输入且等待用户时背景以 ANSI 暗色低频轻柔呼吸，输入、建议或任务运行时保持稳定。`compact_status_line` 默认 true，在最下面一行显示与完全 UI 一致的状态、用量与快捷键，可独立关闭。侧栏、弹窗、附件、搜索和队列照常操作，后台任务与草稿保持不变。

> 最后更新：2026-10-29 | 当前版本：v0.129.0-rc3；「本轮结束」分隔线紧贴上一轮输出，「轮到你」只在聊天区最后一条显示；TUI 完整与简洁模式统一支持点击展开 Thought 秒数耗时摘要；检测到 Runtime 不可用时自动尝试启动一次，`/version` 显示连接或快照失败原因并提供恢复命令；恢复连接后重新允许自动恢复。Thought 折叠摘要与按需展开、思考显示开关、`willdeep web` 与 `willdeep qr` 快捷入口、新会话标识与 `-s <session-id>` 恢复；Provider 与模型两栏联动、自动发现本地 Ollama / Magpie；生成中的输出需在 800 毫秒内连续按两次 Esc 才能中断；流式增量由专用线程分组持久化后广播，活动轮次由 Runtime 独立保存会话检查点；Runtime 交接等待完整终态提交。[交接回归](docs/verification/0.114.0-rc3/README.md) · [流式验收](docs/verification/0.114.0-rc2/README.md)。

模型选择：`/model` 在宽窗口显示 Provider 与模型两栏，打开时聚焦当前模型；Tab 切换栏，↑↓ 与翻页键移动，支持各栏搜索、鼠标点击和滚轮。左栏浏览联动右栏模型，确认模型后才切换实际配置；窄窗口使用 Provider › 模型的两步页面，Esc 返回上级。列表在窗口内缓存并后台刷新，过期请求取消，F5 重试。

新版提醒：TUI 启动后延迟 30–90 秒查询公开 GitHub Release 元数据，此后随机间隔 6–12 小时；跨客户端共享检查缓存和版本去重。运行任务、编辑草稿、弹窗、文本选择或翻看历史时暂缓，空闲后只显示一次普通文字，不响铃、不弹审批、不自动安装。`[ui].check_updates = false` 或 `WILLDEEP_NO_UPDATE_CHECK=1` 可关闭；不向检查接口发送模型凭据、工作区或会话。

输入提醒：本轮结束且没有待发消息时，终端响铃一次；“轮到你”的整个输入区域使用深青色背景，预测提示保持可读，开始下一轮恢复。启动空闲、重复完成事件和后台仍在运行时不会额外响铃，草稿与队列不受影响。声音是否播放由终端自身的响铃设置决定。

流式显示：已完成历史的排版结果复用，增量只处理新内容并绘制可见区域，搜索、工具折叠、宽度和语言变化时重建缓存。活动轮次由 Runtime 保存会话检查点，TUI 只更新内存预览，收尾或正常退出时合并客户端游标。Runtime 全局事件日志保留会话归属，模型上下文仍按会话隔离；新会话首次提交在 Runtime 启动后取得日志头，避免从零重放其他会话历史。连续事件直接走实时队列，缺口才回读日志；预览片段由专用线程以最多 16ms 窗口分组持久化，成功后才广播，关键事件同步落盘。Chat Completions 思考内容首片立即显示，连续小片段按短时间间隔分批。性能对照见 [最终流式验收](docs/verification/0.114.0-rc2/README.md) 与 [首次长会话修复](docs/verification/0.113.0-rc3/streaming-latency.md)。

后台取消与派工：完全访问模式取消当前会话自己持有的 Job 免审批，跨会话和无主 Job 保留确认。只读自动派子 Agent 默认开启，显式关闭仍生效；写入任务仍由主 Agent 按任务包决定派工，不因只读自动派工获得额外写权限。root 侧栏、详情和 Web 时长取当前任务的开始/完成时间，旧记录缺少开始时间时不显示 Agent 存续时长。

延迟命令：以至少 30 秒的明确 sleep 开头时，整条命令自动后台执行；状态区与输入区提示可以继续输入，普通消息在后台等待期间自动发送。原审批、执行顺序与工作目录保持不变，显式超时和任务总预算仍生效；未指定超时时默认包含等待时间并为后续命令留 60 秒，上限 600 秒。短等待、动态等待参数和验收命令不自动转换。

启动会话：裸启动默认询问是否继续当前工作区最近的非空 Rust 会话，5 秒无操作后继续；按键或鼠标操作暂停倒计时，方向键/Enter、Y/N 或鼠标可选择。无历史直接新建。`willdeep config startup ask|latest|new` 保存启动方式，`--session-startup` 单次覆盖，`--new-session` 本次新建；显式 `--resume`、启动提问和非交互调用保持原行为。恢复只读取现有记录并连接 Runtime，不重发旧消息或重新执行工具。

详情关闭：Inbox、后台任务、Agent、Worktree 详情与帮助弹窗的 Esc 关闭提示可以点击，带下划线标识；窄屏提供紧凑 [Esc]，关闭面板不停止任务或改动草稿。

确认菜单：Y/N 等选项各占一行，相邻选项之间留一行不可点击的空白；输入 `#1983#` 并回车可打开本地确认示例，不执行任何真实操作，也不改变运行中的任务。

队列鼠标操作：Ctrl+Q 打开的队列支持点击选择消息，底部固定显示发送、编辑、附件、删除、关闭及快捷键；编辑时切换为保存和取消编辑。窄窗口自动换行，管理期间暂停自动发送，已送达的插话不可撤回。

队首普通文本自动送入当前任务，无需等待后台事件或按 S；当前工具与后台监控继续运行，模型下一步读取消息。打开队列或附件面板时暂停自动发送；命令和带附件的消息保留原有顺序，等待完整轮次结束。审批与提问不会被绕过，取消前未读的消息照常回队。

完全访问审核：命令替换中的未知命令不再等同破坏性操作，网络查询与文件统计免审；嵌套的明确危险命令仍要求确认，智能和严格模式保留审核流程。

Provider 网关接入：统一前台、Runtime 与子 Agent 的 Profile 解析，支持 OpenAI / Anthropic 标准地址环境变量及 api_base_env；Magpie 作为独立 Profile，不替换已有供应商。

Provider 选择：`/model` 顶部显示当前 Provider；Ctrl+P 或点击顶部打开 Provider 列表，方向键/回车或鼠标选择，再选择模型后统一应用。浏览和关闭不改当前配置；任务运行中延后到本轮结束再切。默认子 Agent 继承新连接，仅指定模型的子 Agent 保留模型并继承新连接，显式绑定 Provider 的子 Agent 保持自己的配置；模型列表获取失败时可输入模型名回车。

TUI 鼠标操作：审批按钮保持单行并增加空白间隔，提问选项加入不响应点击的间隔，窄小终端自适应压缩；模型、会话、工作区、命令与媒体列表保持紧凑。

可靠性修复：相同宿主提示保留最后一次，用户输入不去重；思考内容计入上下文，压缩结果校验容量后才报告成功。网络在成功响应头之前最多重试五次，默认等待 2 / 4 / 8 / 16 / 32 秒；原总期限、取消、预算及正文中断不重放边界保持。

Web 智能体工作区：`#agents` 提供紧凑列表、搜索、来源筛选与完整定义管理；`#agent-experiments` 可直接打开 RSI 实验工作台，选择已安装基线、编辑独立候选并运行免费离线验证。实验备注只作为按账号隔离的本机显示信息，不改变 revision、实验契约或评分，也不跨端同步。官方定义需先创建副本；候选不会自动激活或晋升。实验记录没有后台状态或更新时间时不推断，登录只表示账号已连接。Rust Web 沿用每次显式授权 ID 上传；当前没有独立撤销授权入口，应由服务端授权管理撤销，上传仍核验有效授权。

P4b 免费工程补齐：两端共用严格 JSON 四字段输出，独立 Mock 入口经真实 Worker 读取公共夹具并保存执行证据，再交宿主固定夹具 / 参考函数客观核验最小复现。预算诊断保持原字节上界与额度，明确区分本地未准入、已知用量、真实未知用量和取消。原基线、275 份保存回答、13 个预算终态与账本保持不变；新公共夹具只证明工程链路，不能说明候选质量提升。两端 8 个正常案例宿主核验与 6 个资源边界均通过，见 [联合验收报告](docs/verification/0.103.0-rc1/README.md)；运行方法见 [免费 P4b 说明](docs/AGENT_RSI_P4B_FREE.md)。下一轮只登记 [新盲测元数据方案](docs/decisions/2026-10-15-agent-rsi-next-holdout-plan.md)，不启动付费请求。

评分器格式修复与免费回放：纯本地 `report-json-v5` 识别代码片段和行内省略模板，继续拒绝完整多答案与缺损 JSON；`scripts/agent_rsi_format_replay.rb` 不加载账号或网络客户端，校验原审计、逐项摘要及全目录内容后回放保存回答。275 份可评分回答有 79 份格式误拒恢复识别、原通过样本零退化；原始首轮成绩与账本未改，新增请求和费用均为 0。事后回放不是独立质量提升或晋升证据。规则与复跑说明见 [免费格式回放验收](docs/verification/0.102.0-rc9/README.md)。

智能体 RSI P4a/P4b 新增不可变执行血缘与离线定义级配对实验：`willdeep agents rsi` 与 Web 智能体实验区可登记指令候选并通过实际 Worker 和文件工具验收。候选不改变当前安装；每臂工作区、home、验证器和预算冻结，恢复不重复计样本，保留全部尝试及独立验证结论。显式授权后才上传执行摘要；通用实验入口仅允许确定性 Mock；独立 native-trial 入口只通过本机预算代理运行单独批准的冻结模型与定义，不读取用户密钥。模型、题量、实际价格与预算由宿主另行确认。详见 [RSI P4a/P4b](docs/AGENT_RSI_P4AB.md)。

首轮真实实验已完成 288 个计划终态：275 个可评分观察、13 个预算耗尽，708 个物理模型请求全部核账，累计实扣 $0.667158，低于用户批准的整批 $20。两端原生构建、候选和每次额度保持冻结；保留全部费用、基础设施尝试与方法修订，v1/v2 分开报告。最终独立审计 16,253 项通过，盲测只在前 216 项取得可审计终态后开放。Mac 与 Rust 配对差异的置信区间均含零，不支持晋升；80 份 v2 格式门禁失败不能解释为独立审查判断错误，未评测理由语义或泛化能力。见 [脱敏最终报告](docs/verification/0.102.0-rc8/paid-evaluation.md) 与 [补充登记](docs/decisions/2026-10-15-agent-rsi-live-preregistration.md)。

Web 连续工具调用在实时输出和历史回放中默认合并为一行摘要，运行中或失败时最多两行；展开保留全部调用详情。正文、用户消息与思考保持原有顺序，聊天和会话列表使用更宽且清晰的滚动条。

私有智能体同步冲突提供明确的“保留本机并上传 / 采用云端状态”选择。两个分支的版本历史均保留，同版本分叉可填写新版本号；云端删除可以明确恢复或接受删除。解决操作绑定用户看到的本机 revision 与云端 ETag，账号切换或任一端再次修改会中止；本机最终变更在与日常编辑相同的存储锁内完成比较和更新。

Web 输入框键入 `@` 可选择 MCP 服务、智能体、工作区文件（文件最后）；支持搜索、方向键、Enter/Tab、鼠标选择和 Escape 关闭。智能体包含启用且兼容的官方、自定义及当前账号私有定义，选择只插入引用；中文与空格文件路径自动引用，不把邮箱识别成候选请求。

Rust 服务账号浏览器配对与 Mac 使用同一字符集；无效配对或鉴权拒绝直接提示 HTTP 状态，避免让用户反复确认后等待超时。

专业智能体库（P0—P3）：CLI `willdeep agents`、TUI `/agents` 与 Web 智能体库支持自定义定义、签名官方固定版本、导入导出、fork、pin、历史与回滚。some.im 服务账号和模型 Provider 分离，私有定义需要显式加入当前账号同步并主动同步；按 service origin / user / tenant 隔离缓存，冲突保留本地版本。派工使用受基础职责约束的定义快照与技能正文；不兼容定义可见但停用。此阶段未加入 RSI 自动迭代。

命令超时收尾：执行截止后最多等待 1 秒终止与释放资源；Windows 等待 Job 全部进程归零。若收尾失败，错误明确记录，不宣称资源已经释放。

SWE-bench 失败轨迹驱动的运行时强化：原生/显式配置测试入口与工作目录证据、主子 Agent 统一截止和收尾窗口、重复失败保护、异常补丁交付与恢复时保留的实现路径契约。候选独立分支的完整工程与机制验收通过，尚未发布；未将机制回归宣称为新的模型解决率，详细范围与边界见 [运行时强化验收](docs/SWEBENCH_RUNTIME_HARDENING.md)。

团队状态与预算查询共享按目录的增量用量统计：每次查询检查月份文件元数据，未变化时复用结果，追加只解析新完整行；文件替换、截断或删除时重建。跨会话隔离、记录去重和未知/不完整用量预算门禁保持生效。

TUI 流式输出期间输入默认排队，活动区持续显示待发条数和摘要；`Ctrl+Q` / `/queue` 查看列表并编辑、删除或主动立即插话。管理期间暂停自动发送，编辑保留附件和原草稿，发送失败保留消息供手动处理；已送达 Runtime 的插话不可撤回。

运行中提交 `/goal <任务>` 同样排队，轮到它时保存新目标并发送任务正文；不会提前改变当前轮次的目标。提交输入使旧预测失效，回复收尾和预测结果不改写未发送草稿。

界面初始化不占用团队执行锁或恢复子任务，包括已有待恢复任务的旧会话。恢复在取得会话执行权后的首轮执行中进行，每个执行器只恢复一次；后续轮次保留当前子任务状态，团队控制 Turn 同样保留队列恢复和独占执行权校验。

发行包内嵌短剧工坊 0.46.0-rc1、willdeep-config 0.6.0-rc3 与 Favorites 2.3.0。CLI / Web 首次启动静默安装短剧运行包，保留已有同版本和用户数据；插件权限仍由用户在插件中心批准。

Web 插件中心设置先编辑，再点击每个插件的“保存设置”按钮提交；页面显示未保存、保存中、已保存和失败状态。部分保存失败时保留剩余修改供重试，已保存的密钥不回显。

- 模型响应流超时后不自动重放请求；TUI 区分流读取超时与普通超时，提示检查已有改动并续做校验。界面状态复位不代表任务成功。模型重试在发送前拒绝已经过期的请求期限。

- 跨端运行参数 v1：与 Mac 共用完整 JSON、严格校验及 SHA-256 生效指纹；检查点与 RSI 对照门禁核对配置一致性。反馈支持有界待审队列、按需模型异常分流和独立人工结论。契约与执行差异见 [RSI 运行参数 v1](docs/RSI_RUNTIME_PARAMETERS_V1.md)。

- RSI 评测支持受控并发、逐题断点及付费前出处检查；反馈健康报告明确显示缺少的观测和待调查问题。实施状态与后续验收见 [RSI 实施清单](docs/RSI_IMPLEMENTATION_2026_10_08.md)。
- TUI / Web 输入预测在打字时隐藏、删空后恢复，Tab 填入而不发送。

产品概览最多 20,000 个 Unicode 字符；预算内作为参考资料直接注入 Agent 提示，超限后主文档只保留摘要与子文档索引，运行时只注入索引提示并按需读取详情。完整项目指令仍限于适用范围内的 `AGENTS.md` 与 `CLAUDE.md`。

## 项目简介

插件可通过 `willdeep/roundtable/run` 开自定义专家圆桌，短剧工坊可提交六位创作专家的角色、讨论材料和审核结论契约。Rust 宿主顺序调用模型、保留发言与立场、主持人收敛并保存终稿；当前返回完整结果，不提供 macOS 圆桌页面的实时流式展示。

WillDeep CLI 是跨平台 AI Coding Agent 客户端。当前阶段通过用户提供的 API Base、API Key 和模型 ID，在受限工作区内完成模型推理、工具执行和结果验证。

## 核心功能

- 手机运行配置统一四档审批，完整上报实际供应商、模型目录、允许技能和已批准插件；配置先校验后持久化，下一轮实际使用选定模型和技能正文，只读工作区限制保留。详见 [手机配置协议](docs/MOBILE_RUN_CONTEXT.md)。

- 手机与 Web 服务归 Runtime：`/mobile` 已由 daemon 托管；`/webapp` 也改为 daemon 内部 Web 服务，CLI 只发送启停、查询命令。退出 CLI 不关闭服务，Runtime 停止时关闭监听；Runtime 重启按已保存的开关恢复。`/mobile off` / `/webapp stop` 关闭对应开关。独立 Runtime 的进程退出/重启验收已通过，见 [验收记录](docs/verification/0.99.0-rc1/runtime-web-smoke.md)；发布、CLI 安装与 Runtime 升级分别核验。

- 一层多智能体：默认并发 8，可通过 `[agent_team]` 调整并发与队列容量，实际配置按父会话保存；后台派工与重试共用准入。`continue_agent` 保留原 Agent 的历史和权限，每次生成独立 Run；Runtime 空闲会话续做、启动队列恢复和 CLI/TUI/Web 入口已接入。`agent.team` 返回队列位置、本次/累计执行用量及主 Agent、子 Agent、压缩、辅助调用的团队统计。可选团队预算阻止超额后的新请求；在途并发可能超额，用量未知或计量不完整时停止请求。CLI/TUI/Web 提供分页 Run 历史；核心与独立 Runtime 进程验收见 [验收矩阵](docs/verification/0.99.0-rc1/acceptance.md)。

- 多附件管理：TUI `F3` / `/attachments` 查看草稿并保留输入正文，`/attachments sent` 查看当前会话已发送附件；`Ctrl+Q` 待发面板按 `a` 管理所选消息的附件。Web 提供紧凑缩略图列表与统一图标操作，可排序、删除、查看图片大图及编辑文本；已发送附件只读，可引用到草稿。管理待发消息时暂停自动发送。
- Runtime 原生定时任务：关闭 CLI/Web 后仍由 daemon 调度；旧任务保留 ID 和启用意图，插件未启用时导入为暂停。`willdeep schedule`、TUI `/schedule` 与 Web「定时任务」共用配置和历史接口。支持显式时区、单次计划、预算、超时、去重、故障恢复、归档及保留历史；运行结束、目标达成和验证通过分别记录。实际预算与超时验收见 [验收报告](docs/verification/0.98.0-rc1/runtime-management.md)。

- 普通工具循环有只读探索检查点和逐轮预算提醒；重复读取无新证据时提醒收敛，持续新证据时允许继续定向调查。运行器不强制修改，不增加预算或调用；子 Agent / 辅助调用不计入此处的本 Agent token 提醒。实现与验证边界见 [运行时改进报告](bench/swebench/analysis/2026-10-09-runtime-guidance/report.md)。

- 编码修复提示明确原始行为验收、及时执行/委派及工作树内验证；本轮为未发布候选，本地契约测试不能证明模型解决率提升。失败证据与后续 A/B 边界见 [第一轮失败分析](bench/swebench/analysis/2026-10-09-failures-v1/report.md)。

- 内嵌 willdeep-favorites 2.3.0：CLI/Web 启动安装，Web 插件页面支持富文本备忘、图片收藏、搜索与标签；`plugin builtin install favorites --enable` 可明确批准启用，需要 `/usr/bin/ruby`。数据保存在当前 home 的插件私有目录。
- 首次命令行启动时，未配置 Provider 的用户直接收到后台 WebApp 与 willdeep-config 配置地址，保存后自动继续进入 TUI；随包提供配置插件，沿用插件中心的权限审批与启用流程。无 `/usr/bin/ruby` 的环境可用 `--onboarding` 选择终端配置，环境变量已配置的用户不重复引导。

- 手机中继由 Runtime Daemon 托管：`/mobile` 或 `willdeep daemon mobile enable` 打开后持久在线，关掉终端、升级 Runtime 都不断；手机上能看到整个 Runtime 的会话、历史消息与所有待处理的审批和提问，能做的写操作只有四件——发提示词（含图片）、在已登记工作区新建会话、停止当前轮次、批准 / 拒绝审批（只给「这一次」，不给「总是允许」）与回答提问。命令走字面量白名单映射到已有 Runtime 操作，删改会话、登记工作区、改档位与模型一概不开放；手机先答掉的审批，TUI 里的对话框自动撤回。详见 docs/MOBILE.md 与 docs/decisions/2026-09-23-daemon-mobile-relay.md。

- 本机用量账本 `willdeep.usage-ledger.v1`：主回合、子 Agent、上下文压缩与辅助请求（标题、预测、路由、判官、看图兜底）的**每次模型调用**写一行到 `$WILLDEEP_HOME/usage/YYYY-MM.jsonl`，daemon 与进程内回合同一写入口，只记数不记内容，写失败不影响回合；daemon 回合带 `events.ndjson` 序号。`willdeep usage backfill [--dry-run]` 从 daemon 事件日志回填账本上线前的用量，daemon 首次启动自动跑一次。WillDeep for macOS 只读合并进 Token 活动。canonical 规范在 Xedit `docs/USAGE_LEDGER_DESIGN.md`，rs 侧见 docs/USAGE_LEDGER.md。

- TUI 与 Web 轮次结束后预测下一句：空输入框里灰字显示最可能的下一条消息，`Tab` 采用（只填入不发送），打字 / `Esc` / 新一轮开始即清掉；走标题摘要那一档模型的独立小请求，只看最近两条用户原话与助手回复尾部，结果晚到按世代号丢弃；不落盘、不进 Runtime 协议，`[agent] input_suggestions = false` 两端一起关。Web 走独立端点 `POST /api/sessions/{id}/input-suggestion`，不挂进聊天流。详见 docs/TUI_GUIDE.md「轮次结束后的下一句预测」、docs/WEB_GUIDE.md「Composer」。
- OS 级围栏默认开（macOS Seatbelt / Linux bubblewrap）：主 Agent 与子 Agent 的命令、后台任务、监视器、verifier 只能往工作区、临时目录和工具链缓存里写；`workspace-write` 默认允许网络连接供 AI 判官审核后的外部命令使用，`read-only` 断网；可用 `agent.sandbox_network` 显式收紧或放开；`willdeep doctor` 报围栏状态。详见 docs/SANDBOX.md。
- Runtime 控制面可作为平台对外：`willdeep-runtime-client`（Rust SDK）与 `willdeep-runtime-protocol` 发布就绪，README、crate 文档与可运行示例齐全；`willdeep run` 进 CI 的做法与样例见 docs/CI_INTEGRATION.md、examples/ci/。
- `willdeep audit export` 把一个会话（或一个工作区一段时间）里的审批放行、人工裁决、hook 拦截、验证证据、改动归属与回滚汇成一份 Markdown / JSON 审计报告；只读本地状态文件，不需要 Runtime 在跑，报告里没有提示词、模型正文和凭据。详见 docs/AUDIT_EXPORT.md。
- 接住 WillDeep for macOS 交接过来的会话：`willdeep handoff list / accept / watch` 读取 `willdeep/handoff/*` git 分支，切到分支、导入会话并按 `run` 的路径续跑；`watch --accept` 可常驻轮询、逐条接手。信道是 git 远端，两边各用各的凭据。详见 docs/CLI_REFERENCE.md。
- Web 插件宿主提供与 macOS 版同名同序的桥 2.6.0 能力（文件、命令、代发网络、结构化存储、技能清单、生图、问模型时附插件媒体目录里的图片、把文本递给主 Agent、宿主事件；视频附件与流式思考两项仅 macOS 版提供），共享插件包零改动即可在两端运行；本宿主认不出的清单词汇降级显示为「本宿主不支持」而不是拒装；需要原生文件框的插件在 Web 上改由浏览器选文件并上传（参照图 ≤20 MiB；短剧分集背景音乐 ≤200 MiB，v0.84.0-rc1 起）。详见 docs/PLUGINS.md。
- 插件 MCP 与 macOS 版同一套机制（v0.83.0-rc1）：`willdeep web` 在本机开插件 MCP 网关（`mcp-gateway.json`，端口与 token 固定），外部客户端连上即可按需拉起插件，不必先打开插件页；与 WillDeep macOS 同时开着时网关只转发给自己拉起的插件进程，不串到对方（v0.84.0-rc2）；插件进程可反向请求宿主出图、问模型；已启用插件的 MCP 工具进聊天；插件页标题栏、图标与主题变量对齐 macOS。

- Web 插件中心位于左上插件入口之后；语言与主题由图标展开；聊天只显示附件协议边界后的用户正文，选区插件菜单保持到执行或明确关闭。

- Web 与交互式 CLI 保留桌面消息来源与计划状态：宿主指令显示为系统活动，计划/进度块合并为状态卡；CLI 使用 `/plan` 展开或收起最新计划原始记录。展示状态不作为自动验收或停止任务的依据。

- 本地 CLI 与 Runtime 对部分结果统一返回退出码 5，并保留可恢复的结构化输出；真实 DeepSeek 五场景可靠性评测全部通过，报告见 docs/evaluations/agent-reliability-rc58.md；

- JSON 与 Markdown 可靠性评测报告均展示完成率、误报率、恢复率、耗时、Token 及人工介入；未取得的指标不冒充零值；

- 检查点故障注入覆盖工具已写入、结果未保存时取消：磁盘恢复保留未知效果原调用，广泛自动授权不能触发同参数重放；

- 目标循环对重复错误与不变的成功读取升级无进展提示，真实 Agent 回归验证其不会持续重置进展或误报完成；

- 自动压缩有真实 CLI 阈值集成回归，直接核对发送给模型的原始用户约束、近期工具调用配对与持久化用量；

- Web 语言/主题快捷设置位于输入区下方，避免遮挡发送或停止；根聊天重试等待及提交响应到达前停止均有真实浏览器回归；

- 前台 Worker 恢复列表支持超过 4096 条记录的游标分页，分页候选内存保持有界；

- 前台与受管后台验证成功后复核起止文件快照，快照变化或读取失败不会留下可复用的通过证据；

- Web 后台 Agent 重试等待、刷新恢复与停止操作有真实浏览器回归；复现见 docs/WEB_RUNTIME_RETRY_QA.md；

- 手工和自动压缩保留未知副作用的完整工具批次，超限归档不删除原参数及恢复约束；

- 恢复时保留缺失工具结果的未知效果及原参数；重复同名同参数副作用调用需一次性确认，不能沿用自动放行，确认后仍受现有权限和沙箱约束；

- 手工压缩的已报告模型用量随会话持久保存，覆盖 TUI 与非交互入口，失败后保留消耗，续跑不会重复累计；

- 自动上下文压缩的已知模型用量纳入 Agent 预算、结果和检查点，空摘要及备用调用也计入；达到预算上限时阻止后续模型请求；

- Git 验证快照与改动归因纳入子模块内部暂存、未暂存和未跟踪内容，HEAD 不变时也使旧证据失效；

- 主任务与子 Agent 的 Provider 重试等待保存在 Runtime Agent 记录并经公共协议传递；TUI/Web 列表显示等待，开始重试、任务终止或 Runtime 重启时清除，运行中停止入口保持可用；

- 与 macOS WillDeep 共用 `~/.willdeep/config.toml` 的 `[notifications]` Schema：保留桌面通知声音设置，校验本地或远程 HTTP(S) Webhook 地址，并在「任务完成」「需要人处理」两类时机实际投递；除旧 webhook 外可追加任意条渠道（Webhook、飞书自定义机器人、邮件 SMTP / Resend），每条渠道独立过滤事件、独立投递；投递为旁路，失败不影响 agent；Webhook 地址按普通个人偏好落盘，不使用 Keychain，密钥可内联（要求配置文件 0600，`config show` 脱敏）或引用环境变量；
- Chat Completions、Responses、Anthropic Messages 三协议；
- some.im 与 BYOK Provider；
- 共享 `model-catalog.v1` 协议描述 Provider、模型事实、成本与路由候选池，并以 `credential_ref` 将非敏感目录和真实凭据分离；当前已提供严格 Rust 解析/校验与跨客户端接入设计，尚未替换既有 Provider 路由和凭据来源；
- 文件搜索、读取、创建和精确编辑；
- Web Fetch 对每次跳转重做公网目标校验，同域自动跟随、跨域重新审批，并以环路、次数、超时和流式 3 MiB 硬限制约束响应；
- Git 状态、Diff、受限提交历史与逐行归因，以及 Shell 命令；
- 多轮 Tool Call Harness；
- Worker 派工、历史与工具输出保存在私有持久目录；Runtime 重启后续跑同一父会话，后台 Worker 可显式重试，前台 Worker 可用 list_agent_recoveries / resume_agent 查询恢复；沿用原 ID、任务包、写集合及工作树，已有前台完成报告直接交回，不重复工具或 verifier；
- 未完成任务的执行检查点保留按快照与命令区分的验收状态，重建执行器后继续阻止未解决的验证失败；任务级验收条件与完整重启恢复仍在开发中；
- 可通过任务专用配置的 `agent.verification_commands` 指定必须通过的检查；清单在未完成任务检查点中固定，所有检查必须绑定最终 Git 快照，移除配置不能绕过恢复中的要求；自然语言验收项与完整重启派工仍待补齐；
- 连续流式文本按约 250 毫秒或 16 KiB 合并检查点写入，首段、用量和工具执行边界立即保存，正常取消刷新尾部；强杀进程或断电可能丢失尚未刷新的一小段文本，工具副作用记录不采用该延迟策略；
- 持久会话执行所有权覆盖 Harness 提交、后台续轮及本地 TUI 提示词/手工压缩，运行与结果保存期间拒绝同会话竞争执行；Runtime 队列边界与其他写入入口仍在统一验收；
- 工作区路径边界和写操作审批；
- 目录规则在执行前刷新，Shell/MCP/派工的规则发现不受搜索忽略文件影响；根任务与 Worker 共用全局用户规则，规则读取失败或超长时明确报错；
- 命令型子 Worker 使用“确定性静态分类 → 任务上下文 AI Safety Judge → 父 Agent 精确命令人类授权”三级链路；危险形状、凭据内容与敏感路径不进入 AI，Judge 拒绝或不可用时绝不自动执行，`target_command` 的一次性授权也不能被参数拼接或命令替换扩权；
- 人类输出与 NDJSON 自动化输出；
- `willdeep run` 默认通过持久 Runtime 执行，支持 Prompt/stdin、文本与图片附件、Session 续接、断开后继续、text/JSON/NDJSON、静默模式和稳定退出码；`--local` 保留显式进程内兼容入口；
- Bash、Zsh、Fish、PowerShell 补全和 roff man page 从同一 Clap 命令树生成；
- 顶层 `willdeep session list/get/turns/stop` 查询持久会话并精确停止其 active Turn，输出末尾附 `parent=<来源会话 ID 或 none>`；
- `willdeep doctor [--json] [--bundle PATH]` 在不联系 Provider 的前提下生成本地就绪诊断或私有脱敏 ZIP；
- TOML 多 Provider Profile 与安全凭据引用；
- 可选自建辅助模型复用单一 OpenAI-compatible 端点完成会话标题、上下文压缩和低置信度 Worker 路由；支持域名、局域网 IP 与回环地址显式免 Key，各职能独立开关，失败自动回退远端候选，模型路由不得改变关键词规则决定的层级与安全闸门；
- TUI `/routing` 与 Web“模型与路由”设置共用同一套持久化配置：可调整 Root、`reader` / `implementer` / `tester` / `ops_runner` / `judge` / `deep` 六个公开工种、上下文窗口、自动派工与 Deep 预算；旧专门工种仍保留为内部路由兼容 ID；保存带版本冲突检测并原子更新 `config.toml`，可一键恢复 some.im 推荐 Worker 映射；
- 连 some.im / tokenhub 时读取网关模型能力表（v0.85.0-rc1）：看图判定、上下文窗口缺省值、协议回落以网关经探测验证的声明为准，取不到时沿用原有规则；
- TUI 支持 `/model <模型名>` 直接切换当前 Session 模型；裸 `/model` 从 Provider `/v1/models` 获取模型，提供模糊筛选、键盘翻页、鼠标滚动和点击选择，并同步 Runtime 与进程内 Agent；
- TUI Goal 按 Core Session 持久保存，重启以及 Session/Workspace 切换时恢复；`willdeep -r` / `willdeep --resume` 可直接加载最近更新的持久会话，同时保留显式 UUID 与 `latest` 参数；Core Runtime 统一识别 `<goal>` 信封，Web、TUI 与 Headless 共用同一续推判定；
- Provider Profile、模型和配置按 Session 恢复；Skills/MCP 在每轮执行前按当前 Workspace 策略重新绑定，撤权立即生效；
- Daemon 重启后对“无工具活动且历史边界完全匹配”的活跃 Turn 自动重放；已写用户消息原样复用，存在副作用证据或歧义历史时完整保留并停止自动恢复；
- Runtime Daemon 收到关停信号后为控制连接保留 5 秒收尾窗口，超时会强制关闭并释放活租约；过期租约删除失败会返回带路径的明确错误；
- `monitor` 工具盯一条命令的 stdout，新行合批（200ms / 50 行）作为 `<monitor-event>` 在命令还在跑时交给模型，结束、超时、刷屏（60 秒超过 30 个事件）或 `kill_job` 后发 `<monitor-ended>`；审批与 `run_command` 同一道闸，每个监视器 30 秒最多唤醒一次空闲会话，日志完整落盘；格式遵循后台任务合同（金样 docs/contracts/monitor-event.v1.txt）；
- 主 Agent 可用 `send_agent_message`（≤4000 字符，超长拒绝不截断，下一回合边界生效）与 `stop_agent` 指挥本会话起的后台子 Agent，不弹审批但写审批审计；子 Agent 拿不到这两个工具；
- 后台 Shell 由同版本内部 Supervisor 通过匿名帧管道接收命令并监视父 Harness 存活；命令不进入进程参数或 Runtime 资源索引，Unix 在取消、超时或父端断开时终止独立进程组；
- Daemon 重启将运行中的 Child Agent、Tool 与后台 Shell 明确收敛为 Interrupted，未应用 Agent 命令收敛为 Rejected，未真正启动的外部 Spawn Child 收敛为 Failed；后台 Shell 以 `background_shell:<job_id>` 精确绑定 Session、Turn、Task 与 Root Agent，恢复事件仅写一次且只含稳定归属 ID，专属 Worktree 原地保留供后续 Diff/合并/隔离；
- Agent 树累计 input/output/total Token，跨 Session Turn、Child 重试与 Daemon 重启保持；
- Root/Child Agent 持久记录实际模型，统一 API、TUI 与 Web Agent 树展示父子层级、模型、状态、工具、耗时、Token 和 Worktree；
- TUI Agent 列表保持最小摘要，按 Enter 才读取受保护单项详情；详情按 Agent 过滤最近工具时间线与 Workspace Change Artifact，展示已有结果报告，并支持键盘、鼠标滚轮浏览长内容；后台 Agent 可在详情中用键盘或鼠标补充指令、停止、原模型重试、指定模型重试和查看 Worktree Diff，且不会覆盖 Composer 既有草稿；公开 Agent 详情不下发 Prompt 原文；原派工提示词随私有恢复记录保存；
- 统一 `agent.retry` 与 Rust Client 支持为终态后台 Child Agent 指定可选新模型；Harness 在重试边界基于原 Provider 配置重建模型实例，运行中的 Agent 不热切；
- 手动压缩持久记录压缩代次与消息计数检查点；Runtime Fork 仅接受当前压缩代次的精确 Turn 边界；
- `willdeep config init/check/show` 可安全创建、严格校验并脱敏展示 TOML 配置；`check` 以 `legacy` 行点名旧写法（如 `approval = "ask"`），TUI 开屏与 `doctor` 同步提示；
- 零配置启动：没有配置文件时，环境里的 `SOMEIM_API_KEY` / `ANTHROPIC_API_KEY`（或 `OPENAI_API_KEY` + `WILLDEEP_API_BASE`）足以直接开工，Provider 与缺省模型按钥匙推断，不再强制首次设置；TUI 开屏提示凭据来源与 `willdeep config init` 固化方式；
- Ratatui 多轮 TUI、可滚动聊天记录、可点击聚焦并展开/收起的聚合工具活动和界面内审批；审批动作纵向排列，支持方向键加 Enter、Y/A/N、中文“是/否”和整行鼠标点击，输入法提交的无关字符不会触发决定；
- TUI 侧边栏右下角以低对比度常驻显示当前发版版本；`/` 候选与命令分发均识别 `/webapp`；
- 空白新会话的即时工作区欢迎引导；
- 多行 Prompt 编辑使用同一套 Unicode 单元格规则完成换行、滚动、鼠标定位与光标渲染；支持 F2 临时展开为终端大输入空间，并保留文本粘贴附件和可删除图片附件；图片可用 `Alt+V`、`Ctrl+V`、`Ctrl+Shift+V` 或终端可上报的 `Cmd+V` 从本机系统剪贴板附加；
- TUI 聊天区内建 Unicode 可视列选择：可直接鼠标拖选并高亮，使用 Ctrl/Cmd+C 或 Y 复制到系统剪贴板，使用 Q 将选区以 Markdown 引用格式送入 Composer，不依赖终端软件的原生选择实现；
- TUI 与 Web 的简体中文、英语、日语界面及持久语言偏好；
- TUI 聊天区实时落下模型的中途文字与每次工具调用（`· … / ✓ / ✗ 工具 · 摘要`，完成后原地改标记；早先的小轮次收成一行汇总、最新的小轮次一行汇总加最新一条，`/tools` 展开），正文流式增量走临时预览行（「回复中」），思考型模型的思维链只占最后 3 行灰色滚动显示（「思考中」）、正文一开始即隐藏；所有进聊天区的文字先做终端安全处理（制表符展开、控制字符转义），避免残影；会话重开后工具行按记录回放（投影带每条调用的名字、脱敏摘要与有无结果，Web 回放成运行卡片）；系统提示词要求模型在任务开始前和阶段切换时用一句话汇报进度；轮次结束落一行紧贴上一轮输出的「── 本轮结束 · … ──」分隔线并响铃，「轮到你」只在它还是聊天区最后一条时显示，输入框标题常驻「本轮进行中 · 回车即送达 / 轮到你」状态；本轮在跑时按回车先把话直接送进正在跑的这一轮（下一次调模型前以用户身份注入，不打断手上的活），送不到才排队并以「待发」标记回显、发出时改回；聊天标题、活动区和底部状态栏一致的动态运行指示、当前阶段、累计耗时、无新事件等待提示与 Runtime 重连恢复；Web 提供单行工作状态、逐轮工具轨迹（与 TUI 同一套折叠规则：早先的小轮次一行汇总，最新一轮汇总加最新一步，外层再按总步数兜底）、灰色的思维链尾巴行（最后 240 字，正文或工具一来即隐藏）、停止生成，本轮进行中的插话送达（回车经 `/api/sessions/{id}/steer` 转 `turn.steer`，没有在途任务时提示稍后再发），轮次结束落紧贴上一轮输出的「── 本轮结束 · 总耗时 · 工具 N 次 ──」分隔线，「轮到你」只在它还是最后一条时显示，输入框下方提示常驻「本轮进行中 / 轮到你」状态；
- Web 聊天消息、工具轨迹和思考状态使用紧凑垂直节奏；Composer 聚焦只显示单一外框，内部 Textarea 不重复绘制 focus-visible 轮廓；
- Rust CLI 构建显式跟踪 `web/dist`，前端产物变化会触发二进制重新嵌入，Debug 与 Release 不会静默沿用旧 Web UI；
- Web/TUI 独立聊天历史滚动；TUI 支持常用 Markdown、HTML `<br>` 终端换行，以及按中文显示宽度对齐、窄屏单元格换行并带完整闭合细边框的 GFM 表格；
- TUI 将 Markdown 图片渲染为媒体卡片，并以 `Ctrl+L` 汇总当前会话的 Markdown 链接、裸 HTTP(S) URL 与图片；链接通过无 Shell 的系统浏览器调用打开，图片按显式操作安全加载，在 Kitty、iTerm2、Sixel 原生预览与 Unicode 半块字符之间自动降级；
- Rust Runtime Client 为全部公开 Runtime 操作提供类型化方法；Runtime 状态、Workspace、Session、Agent、Turn、Task、Approval、Question、Tool、Artifact、Event、Diff 与 Worktree 控制复用同一协议 DTO，并通过本地 Socket 往返测试约束操作名、Token、幂等 Request ID 与响应解码；
- 统一 `agent.spawn` API 与 Rust Client 可在活跃 Session 中创建稳定 ID 的后台只读子 Agent，并通过 `agent.wait` 观察完成；父级、Task 和 Workspace 均由服务端推导，外部调用不能选择写目标；
- Web Runtime 侧栏按当前 Workspace 展示 Agent、后台 Task、待审批/回答、关注项、Tool 与 Artifact 摘要；进行中与最近五分钟完成的 Task 可查看状态、Profile、耗时、退出码、失败域和归属时间线，Workspace、Prompt、命令、参数、输出、报告、路径、模型、配置、PID 和内部错误不会下发；
- Web Runtime 侧栏可解决三类审批、回答单选/多选/自定义问题，并停止、重试、指定模型重试或补充后台 Agent；写操作重新验证 Workspace 和目标归属，请求体拒绝客户端夹带额外作用域；
- Web Runtime 侧栏可在活动父会话中创建公开 `reader`、`judge` 两种无 Shell、无写入子 Agent；需要命令、写入或 Deep 升级的工种必须由父 Agent 走安全审核、写集审批或升级票据；父级、Task、Workspace 和 Child ID 均由 Runtime 推导，聊天 SSE 输出不会锁死审批、回答和 Agent 控件；
- Web Runtime 摘要默认收起但持续刷新，历史会话默认可见并使用明确的暗色对比度；空白会话不进入历史列表，技能候选提供独立搜索框，侧栏 Footer 展示服务端当前版本；
- Web Agent 与待处理 Task 详情按公开 `agent_id` / `task_id` 展示最近工具时间线、状态、耗时和 Diff Artifact；仅使用已脱敏 Runtime DTO，不下发 Prompt、工具载荷、报告、路径或内部错误；
- Rust Runtime Client 覆盖 Diff 快照、内容、审查、验证、归因、Commit Preview 和安全撤销，TUI Diff Center 直接复用；
- Worktree Review、Merge、Audit、Quarantine 已进入统一 API 和 Rust Client，精确 Review/Snapshot ID 与确认字段继续约束写操作；
- 公共 API 兼容夹具覆盖 Runtime 的 11 类稳定对象，供 Swift、Android 和第三方客户端做跨语言解码回归；
- TUI 状态行常驻 `Ctrl+S 选择` 入口；聊天区也可直接拖动进入内建选区，以 `Ctrl/Cmd+C` 或 `Y` 复制、`Q` 引用，并以 `Esc` 返回普通交互；
- TUI 可从 Runtime Agent 状态分组或 `/agent spawn` 显式创建公开 `reader`、`judge` 两种无 Shell、无写入子 Agent；历史专门工种仍可正常展示和继续已有流程；
- TUI 全局快捷键帮助、Prompt/聊天/活动/状态栏焦点高亮与状态行焦点提示；
- TUI 聊天搜索、高亮与匹配跳转，以及可点击、可滚动的状态栏和后台任务详情；
- TUI `/history`、`Ctrl+R` 和 `/session search` 打开同一个历史会话面板：默认列出当前 Workspace 最近 20 条会话，可按标题或消息内容改词重查并展示命中摘要，方向键选择并以 Enter 或鼠标点击原地进入继续；`/session search` 的 `--status` / `--profile` / `--model` / `--after` / `--before` / `--workspace` 过滤器随每次重查一起下发；已归档会话会先恢复，当前草稿会话状态在切换前保存；`Ctrl+P` 全局命令面板中的当前 Workspace 会话也可直接切换；
- TUI `/new`（别名 `/session new`）在当前工作区开一条不带历史的新会话：沿用当前 Provider、模型与配置，事件游标从 Runtime 事件流当前位置起读，旧会话留在磁盘上可经 `/history` 回去；`/clear` 仍只清空聊天显示，不动会话与上下文；
- TUI 支持 `/uistyle` 在完整 UI 与简洁 UI 间切换，`status` / `no-status` 控制简洁 UI 底部状态栏；运行中也立即生效。
- 折叠工具调用摘要使用更清晰的前景色，避免深色终端背景下难以辨认。
- TUI 在轮次运行中不再吞掉回车：`/help`、`/clear`、`/sidebar`、`/uistyle`、`/skills`、`/history`、`/session search` 立即执行，提示词与 `/local`、`/runtime` 连同附件排队并在本轮结束后按序发出，会改会话或 Runtime 状态的命令给出明确原因；`Esc` 中断当前轮次（Runtime 轮次交给 Daemon 排空，`/local` 轮次掐进程内 Harness），中断后队列立即续上；手机发来的消息不进这条队列，直接进 Runtime 的轮次队列；
- TUI 审批与提问对话框按会话归属弹出：同一工作区开多个 TUI 时，别的会话的审批不再在这里弹出、也无法被就地解掉；无会话归属的任务（headless 提交）仍对所有客户端可解，Web 工作区视图维持原口径；记了发起端的审批弹回发起端，发起端退出后，同一种界面重开同一个会话也能接住，终端与浏览器之间互不代签；
- Runtime 失败工具的原始参数与输出摘要经本机 `task.diagnostics` 提供，TUI Attention Inbox 详情直接显示退出码、失败域、失败原因、失败命令与输出；公共事件流（Web 桥接、手机中继）仍按原规则脱敏，写入日志前凭据打码且输出截断为有界首尾摘要；
- TUI `Ctrl+P` 全局命令面板，可模糊搜索命令、Skills、会话、Agent/任务和工作区文件；
- TUI Prompt、聊天区、活动区和状态栏四态焦点循环，以及候选、审批和 ask_user 的鼠标操作；
- Core 统一 Agent、后台任务、审批与提问的运行状态、Attention 分组和父级状态聚合语义；
- TUI 右栏 Attention Inbox 按人工介入优先级聚合待处理、工作中和最近完成事项；
- TUI 侧栏手动滚动使用饱和范围和延迟命中计算，极小视口、超大滚动偏移及不可见命中行不会触发整数下溢或退出 TUI；
- Attention Inbox 支持键盘/鼠标选择、后台任务与子 Agent 详情跳转、停止运行项和标记已读；
- Diff Inbox 详情独占键盘事件，支持 D/Enter、Y、N 与鼠标点击查看完整 Diff、整批通过或拒绝；决定绑定提交时的精确快照，冲突状态禁止整批通过。统一 Diff API 对旧 Runtime 的快照、内容和审查路由安全回退，操作错误只显示提示而不退出 TUI；
- Diff Review 模态独占全部鼠标事件，滚轮仅浏览当前 Diff、文件列表或 Commit Preview，不会穿透并滚动底层聊天区；
- Diff 文本进入 TUI 前展开 Tab 并转义 ESC、响铃等终端控制字符，返回列表或关闭模态时强制完整重绘，避免终端光标与 Ratatui 缓冲失步后在聊天区留下残影；
- Inbox 已读状态随会话持久化，后台 Shell 与子 Agent 支持真实重试，任务结束时触发终端提示；
- Git 冲突和待审 Diff 使用内容指纹进入 Inbox，子 Agent 审批阻塞结构化上报，状态按 Agent→会话→Workspace 上卷；
- 跨平台 Runtime Daemon 提供 `start/status/stop/logs/upgrade`；Upgrade 以 draining 闸门拒绝新工作、保留活跃任务和持久排队 Turn，并只按活动 Task 状态等待排空，终态 Task 遗留的 cancellation 句柄不会阻塞接管。TCP 与本地 Socket 共用有状态广播关闭信号，晚订阅监听器也不会漏掉通知。Headless 观察者识别 Runtime 身份更替后沿事件游标自动重附着；本机客户端优先通过权限为 `0600` 的 Unix Socket 或拒绝远程客户端的 Windows Named Pipe 通信，并兼容旧状态的受 Token 回环 TCP；
- Session/Turn CLI、TUI 与 Web 共用统一 Runtime Client；收养既有 Session 时由 Runtime 从 Core 存储恢复私有配置引用，公共协议不传输配置路径；
- macOS 桌面会话桥接保留 assistant 工具调用与 tool 结果的关联 ID；Provider 重放前会清理旧版本遗留的孤立工具记录，使已经导入的坏会话也能继续使用；
- Runtime Session 元数据使用可迁移 schema；schema 1 首次由 rc33 打开时先写私有原始备份，再原子升级为 schema 2。新建未命名会话以首次 Turn 生成本地有界标题，疑似凭据 Prompt 不复制到标题；显式 Rename、收养旧会话和 Fork 标题保持人工所有权；
- Runtime 持久记录主 Agent 与子 Agent 的 Tool Activity，支持按 Session、Turn、Task、Agent 和状态查询；公开记录只含工具名与生命周期，不含参数、输出或 Workspace 路径；
- 工具窗口内经内容指纹确认的真实文件变化会形成 Workspace Change Artifact；公开元数据包含来源快照和变更数量，文件路径与内容继续由受授权 Diff API 控制；
- TUI 右栏通过统一 API 显示当前 Workspace 的结构化 Tool/Artifact 数量、运行态和最近工具，不从聊天文本反推持久状态；
- Web 侧栏通过 Workspace 白名单约束的 Activity API 周期刷新结构化 Tool/Artifact 摘要，支持中英日显示；
- Runtime 事件以 NDJSON 和单调序号持久化，`attach --after` 支持按游标补读并安全分离客户端；
- Runtime 事件日志读写使用同一互斥边界，活跃追加期间的控制 API 不会读取到未完成的 NDJSON 末行；
- Runtime 提供受 Token 保护的 SSE 事件流，按游标分页补历史后切换实时广播；慢客户端从持久日志恢复，TUI 长连接消费并对旧 Daemon 保留轮询降级；
- Runtime drain/shutdown 会广播关闭状态给 SSE 与 NDJSON 事件流；观察连接主动结束并由客户端沿持久游标重附着，不阻塞 Daemon graceful shutdown；
- 非交互 Harness 可通过 Runtime 提交、查询和取消；Daemon 直接持有进程内 Harness Future，并把模型输出、session_id 和终态写入可续传事件流，不再为每个 Turn 启动 CLI 子进程；
- Headless CLI 默认创建或续接同一 Runtime Session/Turn，按游标分页追平脱敏持久事件并按失败域维持自动化退出码；进程级敏感覆盖保守留在 `--local` 路径；
- CLI 与 Runtime 共用 Provider、视觉降级、审批、Skills、MCP、Tools、子 Agent Profile、会话写入和后台结果回流的 Harness Factory；Agent 事件直接写入 Runtime EventLog，不经过 stdout 文本中转；
- TUI Turn 提交优先使用统一控制 API；旧 Runtime 明确返回 404 时，以同一 request ID 回退到旧 Session Turn 路由，避免重复提交且不自动重启 Daemon；
- Runtime 使用带心跳的单实例租约锁协调并发启动；异常退出后一次启动请求可在旧租约过期后安全接管，重启时将遗留 Running/Waiting Task、Turn、Agent 标记为 Interrupted、取消 Pending Interaction，并补写可续传恢复事件；
- Runtime 优雅停止会先取消并收敛进程内 Harness Future，再关闭 HTTP Server，等待审批或回答的请求不会阻塞 Daemon 退出；
- Runtime Session/Turn API 与 CLI 提供稳定 Root Agent、幂等请求 ID、持久严格串行队列、排队/运行取消、终态事件和重启恢复；成功后 Core Session 保留唯一消息历史并清除队列私密正文；
- Runtime Session 支持 Rename、完整消息快照 Fork、Archive/Unarchive、精确确认 Delete、安全 JSON Export 和标题/消息 Search；活跃/排队会话禁止破坏性管理，Fork 不复制 Turn/Task/Interaction/游标/已读状态，Export 不包含队列私密正文或凭据；
- CLI、TUI `/session` 与 Web 会话侧栏接入统一管理 API；支持按已完成 Turn 的持久消息边界精确 Fork、Fork 时覆盖 Provider Profile/模型、组合搜索，以及 TUI 在同一 Workspace 内原地切换并按 Session 隔离聊天事件；Web 仅在已允许 Workspace 内操作，匿名 Web 不开放消息摘要全文搜索端点；
- 可选 Herdr 适配器从 Runtime Task 聚合状态并上报当前 Pane，提供不泄露 Socket 路径的诊断命令；Herdr 仅作终端承载和状态投影，不成为 Harness 状态来源；
- TUI 普通 Prompt 默认幂等收养当前 Core Session 并提交长期 Session Turn，`/runtime` 为明确别名、`/local` 为单轮兼容入口；即时展示输入，终态从 Harness 独占写入的 Core 历史同步，恢复 TUI 后继续同一 Root Agent 且不会双写消息；
- Runtime 托管任务支持持久审批和 ask_user 待处理项，可由其他 CLI 客户端允许、拒绝或自由回答后继续原 Harness；
- TUI 右栏统一展示 Runtime 任务与待处理项，支持远端审批、回答和停止；Composer 可用 `/runtime` 提交含文本或图片附件的可分离任务；
- TUI 按 Session 事件游标补读当前 Workspace 的 Runtime 模型、工具、用量与完成事件；用户请求和正式回复写入会话，退出重连后完整恢复且不重复；
- TUI 对话区仅展示用户输入、AI 正式内容和必要错误；Runtime Turn/Task/Agent ID、轮次与提交确认只进入状态栏、右栏或诊断接口；
- Runtime 持久维护 Root Agent 的 ID、父子关系预留、Profile、状态、轮次、当前工具与 Token；受 Token 保护的 Agent API/CLI 可查询，TUI 右栏显示当前 Workspace 的 Agent 摘要；
- `spawn_agent` 通过稳定 UUID 上报启动、轮次、工具、用量和完成事件；Runtime 持久建立 Root→Child 树，TUI 分层展示前后台子 Agent 的实时状态；
- 子 Agent Profile 支持 Token 总预算、执行超时和连续失败熔断；成功任务自动复位失败计数，Runtime 持久化策略快照，TUI 同行显示当前/最大轮次、已用/预算 Token 和时限；
- 子 Agent 结束时将最多 64 KiB 的报告随结构化生命周期写入 Runtime Agent；CLI 单 Agent 详情与 TUI Enter 详情层可查看，报告不会进入公开候选接口；
- 运行中的后台子 Agent 支持追加父级指令；Core 在下一次 Provider 请求前注入并在结束竞态时继续下一轮，Runtime 命令正文投递后立即清除，仅保留脱敏命令审计；
- Runtime 为已注册 Workspace 生成包含 HEAD、文件状态、暂存/未暂存范围、二进制标记、增删统计和内容指纹的 Diff 快照；读取文件 Diff 必须携带当前快照 ID，变更后拒绝陈旧审查结果；
- CLI `daemon diff-snapshot`、`daemon diff-file` 与 TUI `/diff` 共用受 Token 保护的 Review API；TUI 支持文件导航、滚动和 Unified Diff 语法着色；
- TUI Diff Review 可用 `V` 切换 Unified/Side-by-side、`S` 循环 Combined/Staged/Unstaged，并通过 `/` 搜索当前文件、Enter/Shift+Enter 或 N 前后跳转匹配；
- Diff 审查决定以精确 Snapshot/Workspace/文件为键持久化，支持 Accepted、Rejected、Changes Requested 和 Reviewed；TUI 用 A/D/C/M 操作并在文件列表显示结果；
- 单文件安全撤销必须通过快照一致性检查和 TUI 二次确认；tracked 内容按选择范围恢复 Index/Worktree，untracked 或 HEAD 中不存在的内容移入 Runtime Recovery 目录而非直接删除；
- `run_command` 自动识别 Cargo/Go/Python/Ruby/Swift/Node/.NET/Java/Make 等测试命令，在完成瞬间把命令、退出码、通过/失败/超时状态与最多 8 KiB 摘要绑定到精确 Diff 快照；前后台命令共用协议；
- Runtime Verification API、CLI `daemon diff-verifications` 与 TUI Diff Review 可查询绑定结果；普通 Shell 和疑似包含 API Key、Token、Secret、Password 的命令不会持久化；
- Runtime Commit Preview API、CLI `daemon diff-commit-preview` 与 TUI Diff Review 的 `P` 面板共用精确快照；预览提交消息、分支、暂存/未暂存文件、脱敏 Remote、推送目标与可选 Tag，并在敏感文件/凭据、冲突、空暂存区、Detached HEAD 或无效目标时返回阻断原因；该阶段只预览，不执行 Commit、Tag 或 Push；
- Runtime TUI 对话记录只保留用户输入与 AI 最终回复；轮次、Task/Agent ID、工具和排队状态保留在活动状态层，不污染聊天历史；
- Runtime 在 `create_file`、`edit_file`、Shell、Worktree、Computer Use 和 MCP 等潜在写工具的调用窗口前后采集工作区内容指纹，持久记录 `Session → Turn → Task → Agent → Tool → Paths`；主 Agent 与子 Agent 使用各自身份，已有脏文件只有在窗口内真实变化时才归属；
- Diff Attribution API、CLI `daemon diff-attributions` 和 TUI Diff Review 沿快照链显示文件最近责任 Agent 与工具；记录采用 Tool Window 置信度，共享工作区并发的强隔离由后续独立 Worktree 阶段完成；
- TUI Inbox 自动隐藏完成或取消超过 5 分钟的 Runtime 任务；等待审批/回答的任务与其 Interaction 建立直接关联，鼠标点击或 Enter 可进入实际审批/回答控件；
- Runtime 持久维护多 Workspace 注册表，提供注册、更新、列表、激活和保守移除 API/CLI；每项独立保存规范化根目录、访问策略、默认 Provider、Skill 与 MCP 允许列表，切换默认项不影响旧 Workspace 任务；
- 审批档位与 macOS 版逐档对齐：`strict`、`smart`、`workspace-write`（默认；工作区内写入由 OS 围栏保护并免审，工作区外、远程与联网命令走 AI 判官）、`full-access`（除破坏性命令黑名单外免审）；TUI 用 `/permissions`（确认页防误开完全访问）或 Shift+Tab（只循环前三档）随时切换，对正在跑的 Runtime 轮次立即生效，`/permissions default` 写回配置，每次切换记审计；
- Runtime 在任务入队时以服务端注册表覆盖客户端 Workspace 策略，会话选过的审批档位优先、`read_only` 工作区为硬上限；只读 Workspace 在审批前阻止 Shell、文件写入、Worktree、MCP 与 Editor 子 Agent，默认 Provider 和非空 Skill/MCP 允许列表进入同一 Harness；
- TUI `/workspace` 打开工作区面板：一行一个工作区（名称与路径在前，ID 不占版面），当前工作区高亮且默认选中，输入即过滤（名称/路径/ID），↑/↓/PgUp/PgDn 选择，Enter 或鼠标点击原地切换，Esc 或点面板外关闭；`/workspace list` 仍输出含 ID 的文本清单，`/workspace switch <id|名称|路径>` 接入 Runtime 注册表；切换保存/恢复 Workspace 专属 Session 与事件游标，重启事件订阅、状态和 Skill 视图，不取消旧 Workspace 后台任务；启动时绑定旧路径的 `/local` 在跨 Workspace 后保守禁用；
- Web 工作区 API/选择器改读 Runtime 注册表，并与服务启动时的路径白名单取交集；展示当前项与 read-only/smart/workspace-write 模式，Composer Skills 使用 Workspace 允许列表；默认 Workspace 内文件写入免审，Shell/MCP/网络仍按审批策略执行；
- 内置 Editor 子 Agent 默认创建 `willdeep/agent-<id>` 专属 Git Worktree；已审批目标按根工作区相对路径映射，Runtime 持久显示实际目录、根目录和分支，Diff 归因在 Child Worktree 内采集，任务结束后保留供审查；
- 子 Agent 完成报告结构化回流有界 Worktree 状态；Runtime/CLI/TUI 提供两阶段 Worktree Review 与显式合并，Review ID 同时绑定 Child/Root 快照和二进制补丁，任一侧变化即拒绝陈旧操作，同文件冲突、未跟踪内容、未解决冲突和超大补丁均阻断；
- Runtime Worktree 审计识别 Active、Reviewable、Merged、Clean、Quarantined、Missing 和无 Agent 记录的 Unknown 目录；保守清理仅允许精确快照的终态干净/已合并 Worktree，并整体移动到 Recovery，保留所有内容、Git 关联和分支；
- 独立 `willdeep-runtime-protocol` crate 定义协议版本、Runtime/Workspace/Session/Agent/Turn/Tool/Task/Approval/Question/Artifact/Event 对象类别、稳定操作名、能力、传输类型、限制及统一成功/错误信封；Runtime 提供受 Token 保护的能力协商端点和 CLI；
- Runtime 提供统一 `POST /v1/api` 与可续传 NDJSON 事件流；修改类请求按 Request ID 有界去重，共享 `willdeep-runtime-client` 负责回环校验、鉴权、信封解析和流式解码；
- Session、Turn、Agent 与 Event 使用协议 crate 的公开 DTO；TUI 实时事件、Agent 观察/控制、审批和回答均通过共享 Runtime Client，公共事件边界兼容净化旧日志中的工具参数、输出、报告、路径和内部错误；
- Task、Pending Approval 与 Pending Question 也使用公开 DTO；TUI Inbox 和 Web/TUI 事件补读通过共享 Client，修改请求以 Pending→Completed 私有日志跨 Runtime 重启去重，不确定崩溃窗口拒绝自动重放；
- CLI 的 Task/Agent 列表、详情、停止、重试、补充指令、审批、问答与 Diff 命令均通过共享 Client；TUI Turn 提交、事件 Task 归属与 Diff Center 不再回退旧资源路由，旧 Runtime 必须先通过受控升级接管；
- 进程内 Harness 的完整 Task 提交、长等待 Interaction、Agent 命令轮询/回执和 Daemon drain/shutdown 迁入 `/v1/internal` 私有传输；除 Runtime Token 外必须携带内部标记，缺失时以 404 隐藏端点，公开 Client 不提供这些方法；
- Workspace 与访问模式使用公开 DTO；注册、自动确保、激活、移除和 CLI/TUI/Web Workspace bridge 均通过共享 Client，服务端继续规范化路径并覆盖任务权限；
- 统一 API 的全部修改操作使用单一可测试清单进入 Pending→Completed 持久幂等日志；不确定崩溃窗口拒绝自动重放，删除/移除返回稳定对象结果，Runtime 状态可区分正常与安全排空；
- Diff Snapshot、File、Review、Verification、Attribution、Commit Preview 与 Revert 使用协议 crate 的稳定 DTO；TUI Diff Center 的全部读写均通过共享 Runtime Client，精确快照、Workspace 授权、敏感命令过滤和 Recovery 撤销语义保持不变；
- Web Session 列表、重命名、指定 Turn Fork、归档/恢复、精确确认删除和显式导出通过统一 Runtime API 与共享 Client；浏览器仍先校验启动时 Workspace 白名单，公开 Session DTO 不返回配置路径、内部错误或排队 Prompt；
- TUI Session 搜索以及 TUI/Web Turn 提交与停止通过统一 Runtime API；文本和图片附件使用协议 crate DTO，服务端强制附件数量、文本长度、图片 MIME/尺寸、总载荷和 1 MiB Prompt 上限，客户端不能在 Turn 请求中覆盖 Workspace 或权限；
- TUI Prompt 可用 `/webapp` 在当前 Workspace 启动仅监听回环地址的内嵌 Web App，并以 `/webapp status` 查看地址和进程状态；启动继承当前配置/Profile，并等待健康检查成功后才报告完成；
- 后台子 Agent 的取消、失败和重试沿用稳定 Agent UUID；Runtime 通过受 Token 保护的持久命令队列向原 Harness 下发精确 stop/retry，CLI 与 TUI 均可操作并查看结果事件；
- Web 文本/图片粘贴附件、发送前删除、`/` 命令和 `$` 技能候选；
- Web 与 TUI 共用持久 Runtime Session/Turn；Web SSE 事件携带单调游标，浏览器保存最后会话与活动 Turn 游标，刷新或普通网络断开后从持久历史自动重新附着同一 Turn，按游标去重且不重复提交 Prompt；停止操作仍精确终止该 Turn；
- Web 恢复请求与 Turn 完成竞态安全：若断线期间活动 Turn 已进入终态，服务端从最新持久 Turn 和 Core Session 返回最终消息，而不是要求仍存在 `active_turn_id`；
- JSON 会话持久化、列表与恢复；
- Codex 兼容 Skills 发现和按需读取；
- MCP 工具发现、注册和调用，stdio 与 Streamable HTTP 两种传输，远程服务支持静态 Bearer 与 OAuth 2.1 登录（`willdeep mcp login`）；工具 Schema 不再全量常驻每轮上下文，通过 `list_mcp_tools` 按需搜索、`call_mcp_tool` 精确调用；
- `/goal <目标>` 保存持续目标并立即启动首轮执行，后续消息继续携带目标；`/goal off` 关闭。Web 与 TUI 行为一致；`$skill-name` 可显式触发技能；
- 分阶段 CLI/TUI/Runtime 产品路线图与逐项验收状态；
- `/mobile` 出配对二维码并打开 Runtime 托管的手机中继（见本节第一条）；
- 区分角色的 TUI 配色；
- macOS Universal、Windows x64、Linux AMD64/ARM64 自动构建与 tag 发布。
- `rg` 优先、内置扫描兜底的跨平台文件搜索；
- some.im 纯文本模型的 `qwen3-vl-plus` 图片描述降级链路；
- 受审批保护的网页搜索和公网网页正文读取；
- 上下文用量、Token（K/M 两位小数）、缓存命中率、耗时、自动摘要压缩及宽屏后台状态侧栏。
- `/compress` 手动压缩当前会话上下文并立即保存；
- 后台 Shell Job 与完成/失败后自动唤醒主 Harness 的结果回流；
- `spawn_agent` 前台/后台子 Agent，公开 Reader、Implementer、Tester、Ops Runner、Judge、Deep 六个工种；旧专门 ID 继续承载自动路由、托管链与历史兼容。Runtime 将高置信度只读请求自动派给 someim-32b 托管工种，Root 默认使用 GLM-5，Deep 必须通过升级票据和每 Harness 调用预算；
- Task Packet 结构化派工（目标 / 已知事实 / 约束 / `read_files` 上下文 / `write_files` 精确权限 / 验证命令）与 Verifier 闭环：验证命令由 Runtime 亲自执行，退出码是唯一裁决，失败输出消化后回灌重试，尝试打满即判失败并要求升档；
- 写入型工种的文件集写通道：最多声明 16 个现有或待创建文件，`smart/workspace-write` 继承主 Agent 工作区写权限，`strict` 一次审批整个集合，越界拒绝并给出扩权路径，运行中文件集互斥；
- 测试/构建命令失败时在工具结果尾部追加确定性派工提示（仅主 Agent 可见）；
- 子 Agent 判定遥测：每次运行落盘验证结果、尝试次数与起始 commit 锚点，`willdeep daemon agent-metrics` 给出 Worker/Standard/Deep 实际运行数、Deep Share、Skill Coverage、Worker Verified Success 与 Escalation Rate，「未验证」是独立于通过与失败的第三种答案；`--json` / `--since` 供每周快照脚本取数，快照归档到 `bench/agent-metrics/`，趋势写回 README 与 `docs/AGENT_METRICS.md`；
- TUI 右栏实时后台任务状态、耗时及输出查询/取消工具；
- Core `ask_user` 候选单选/多选与自由输入交互；
- Allow once、Disallow、窄作用域持久 Always Allow 审批状态机及规则管理命令；
- 首次运行交互式 onboarding 与 some.im 浏览器登录；
- Swift Project 元数据和历史会话兼容读取；
- Git diff/worktree 原生工具与 TUI 工作区状态；
- 全局、项目根 `AGENTS.md` / `CLAUDE.md` 指令加载；
- React + Chakra UI 纯 CSR、SSE 进度、多工作区切换和并发限制的内嵌 Web 服务；

## 技术栈

- Rust 1.94；
- Tokio 异步运行时；
- Reqwest + rustls；
- Clap CLI；
- Ratatui + Crossterm TUI；
- Serde 协议编解码；
- ignore、regex、globset 工作区搜索。

## 项目结构

```text
crates/willdeep-core/   Agent Loop、Prompt、Provider、工具
crates/willdeep-cli/    参数解析、审批和终端事件输出
config.example.toml      Provider/Profile 配置模板
docs/                   架构与协议说明
```

## 运行

```bash
SOMEIM_API_KEY='<your-key>' cargo run -p willdeep -- \
  --provider some-im \
  --model glm-5 \
  --workspace . \
  '检查项目状态'
```

生产使用时应通过环境变量或后续凭据存储提供 API Key，避免出现在命令历史中。

## 已知问题与后续

- [ ] Provider 原生 token streaming；当前 SSE 已实时传输 Harness 阶段、工具进度和最终回答。
- [ ] ACP/Codex App Server/Goose 接入；
- [x] MCP Streamable HTTP 与 OAuth（v0.78.0-rc24；服务端主动请求与 GET 事件流未接）；
- [ ] LSP 诊断、Hooks/插件、自动记忆与 OS 级 Shell 沙箱；当前已有结构化工具门禁、工作区边界和小模型路由，但这些仍是与 Claude Code 完整能力面的主要差距；
- [x] 手机端工具审批与提问回答（v0.82.0-rc1，中继归 Runtime）；跨设备 Patch 审核 rs 侧没有「补丁提案」对象，写入走审批，暂不做；
- [ ] 手机中继端到端加密：中继服务端目前看得到明文（与 macOS 版同一边界），设计见 Xedit `docs/MOBILE_GATEWAY_E2E_ENCRYPTION_DESIGN.md`；
- [x] 平台沙箱：写入围栏默认开、网络围栏按档位断通（v0.78.0-rc25）；更强的命令风险分类仍待做；
- [x] 小模型路线做成数据：`agent-metrics --json --since` + 每周快照发布 Deep Share / Worker Verified Success（v0.78.0-rc26，`docs/AGENT_METRICS.md`）；
- [x] 检查点回退：每轮开始前工作树快照进私有影子 git 仓库，`/rewind` / Web ↶ / `session.rewind` 回到第 N 步，对话与文件可一起回（v0.78.0-rc28，`docs/CHECKPOINT_REWIND.md`）；
- [ ] 流式真实 reasoning 摘要；当前单行区域显示可验证的运行阶段，不伪造模型思考内容；
- [ ] Swift/Rust 共享会话 schema 稳定后开放双向原地写入；当前采用安全副本。
- [ ] 抽取 Swift/Rust 共用的签名 Computer Use Helper 协议，再开放 AX 检查与短效控制租约。
- [-] 将 Harness、任务和会话生命周期迁入 Runtime Daemon；持久 Session/Turn、attach/detach、事件断线续传、TUI/Web Session/Turn 及 Runtime 进程内 Harness 已完成，跨重启恢复活动执行资源和统一客户端 API 尚待完成。

会话定时任务（v0.100.0-rc1）：聊天中创建时绑定当前会话并在到点后续跑；原会话忙碌时跳过。Web 聊天右侧可新建、暂停/恢复并查询运行历史。旧独立任务继续使用独立会话。定时循环提供检查与改进的执行机制，质量提升仍需实际验证结果。
