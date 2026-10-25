# 三集自动剧本试点

日期：2026-10-25。状态：本地开发，未提交、未发布。

Runtime 持久调度策划、独立审核、三集写作与修订、最终通读和导出。Video Studio 保存正文、候选、场次、问题及证据；Runtime 不复制正文数据库。正常对话和插件页面入口需要配套 Rust `0.118.0-rc1`、插件 `0.51.0-rc1`，均为本地未发布候选。

## 正常对话和插件界面

在已启用并批准工坊的普通对话里输入「返乡修理铺里的祖孙亲情，温暖、克制，三集讲完，请启动自动剧本」。模型通过 `list_mcp_tools` 的 `screenplay` 关键词发现命名空间工具 `screenplay_start` 并调用，沿用宿主现有首次工具审批；后续各集不逐次确认。任务持续运行，普通对话可以通过 `screenplay.inspect` 查进度、`screenplay.download` 取定稿。

插件侧栏的「自动剧本」提供相同入口，只需填写大概选题。页面显示默认累计上限 50 万 token /64 次，可在启动前调整；工作区由宿主白名单限制，多工作区时显式选择。聊天启动强制采用当前会话工作区，覆盖模型伪造路径。启动请求编号先经宿主存储写入，传输失败或刷新后沿用同一编号与原参数确认，不重新建剧。

看板显示每集写作、编辑/合规审稿、修订次数、必改项、阶段历史、实际调用与 token、尚未结算预留。离开页面只停止查询，不取消任务。暂停后可显式增加累计额度，再独立点击继续；取消停止后续派发，保留已发调用的结算与稿件。未知计费状态继续阻止恢复。

定稿可下载 Markdown、JSON、验收清单；读取要求 Runtime completed、当前内容指纹匹配、manifest 与文件摘要匹配，页面再次验证 SHA-256。浏览器发起 Blob 下载，沙箱只为工坊开放 allow-downloads，仍不开放 allow-same-origin、表单或弹窗。旧宿主返回清晰的不支持提示，不自动启动新的 Ruby 循环或升级 Runtime。

```sh
willdeep screenplay start '乡村创业，温暖但有悬念，三集闭合' --workspace /path/to/workspace --max-calls 64 --max-tokens 400000
willdeep screenplay status WORKFLOW_UUID
willdeep screenplay list
willdeep screenplay cancel WORKFLOW_UUID
willdeep screenplay resume WORKFLOW_UUID
# 仅在用户明确授权新额度后，按旧额度做冲突检查：
willdeep screenplay budget WORKFLOW_UUID --expected-calls 32 --expected-tokens 200000 --max-calls 64 --max-tokens 500000
```

任务经 Runtime 控制面运行；启动命令返回后继续，不依赖 TUI/Web 常驻。`screenplay.start/status/list/cancel/resume` 也可经通用 API 调用。状态及模型响应回执存于 `$WILLDEEP_HOME/runtime/screenplays/`，文件私有。任务 grant 只在 Runtime 和插件之间传递，不对公共状态输出。

预算调整保留所有用量、任务与修订记录，并追加调整历史；不会自动提高额度。预算保护后可以在原额度内显式 resume，派发前仍重新检查预留；额度不足则继续停止。增加额度必须先获用户明确授权。

场次使用 beats 顺序表达动作与台词，避免整场动作被排在全部对白之前；兼容已有候选的旧格式。

三集为本试点固定范围；不生成图片、语音或视频。写作与审核沿用插件的 assistant/review 模型设置和宿主 Provider 配置。两个编辑先独立审读，必须关闭有证据的必改项，再由最终验收编辑覆盖全部三集并验证重要伏笔。格式错误最多纠正两次，调用也计入预算。

预算包含本流程每次模型调用和失败尝试；派发前按请求字节与输出上限预留额度，再按 Provider 用量结算，为最终通读保留额度。不提供精确金额预算。用量或执行结果未知时阻止新的付费调用；有可回读响应的原请求按任务身份和请求摘要复用，不重新扣费。未知且无回执的调用需人工对账；不会自动重放。

插件在内容变更前后检查 Runtime 授权，提交时检查输入摘要和人工草稿。修改早期候选使后续集的审核失效。导出 Markdown、JSON、SHA-256 manifest，只有当前稿、全部审核与伏笔证据一致且无必改才标 completed。

协作模式与旧 `drama.write_with_panel` 保留。CLI 专用控制命令和普通对话/插件页面共用同一 Runtime 状态；通用 Goal/Worker 专属集成、12/30 集扩展、并行编剧、货币预算与人工编辑后的自动任务重排属于后续阶段。

本地验证必须使用独立 `WILLDEEP_HOME`，并给插件设置独立 `VIDEO_STUDIO_DATA_DIR`。不得为验证替换现有 Runtime。插件导出位于数据目录的 `screenplay-exports/<workflow>/<snapshot>/`。

2026-10-25 调试补充：Chat Completions 与 Responses 均透传输出上限。剧本 DeepSeek 辅助请求显式使用非思考参数；隔离样例选用同一 Provider、同一模型的 Chat Completions 路由验证，用户默认配置不变。DeepSeek 参数依据：[官方接口文档](https://api-docs.deepseek.com/api/create-chat-completion/)。

2026-10-25 真实验收：《待取》原任务 `d2ad3d58-a1d0-4d8d-9095-c045403dd314` 已 completed。累计35次真实模型调用、521,096 token，三集正文、4次修订事件（策划1次、正文3次）、独立复审及最终伏笔证据保留。全部导出摘要回读一致，无未解决必改或未知用量。100万token/64次上限由用户明确授权，既有记录未清零；默认Runtime仍保持原进程，不提交或发布。
