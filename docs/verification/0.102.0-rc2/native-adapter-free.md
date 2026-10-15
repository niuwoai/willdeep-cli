# 原生 Worker 评测适配：免费验证

版本 `0.102.0-rc2`，2026-10-15。配套报告：[native-adapter-free.json](native-adapter-free.json)。本轮未调用真实模型、未读取用户推理密钥、未改生产或发布客户端。实际价格补充确认、可信预算准入及最终付费执行由根控制器管理。

- 抽出的生产 Worker 执行接口仍由真实工具、反馈与执行事件记录实际定义 revision、最终提示词、模型及根 / Worker 血缘；接口不接题目 ID 或 oracle。
- 核心 verifier 输入组 1 项、共享 RSI 组 6 项通过。CLI RSI 中间完整快照 21 项通过（原有 17 项及当时 4 项 native 测试）；读取证明补强后的最终 native 组 5 项通过，不把不同快照的数量相加。
- 最终全 workspace / all-targets Clippy 与 Rust 构建通过。Clippy 曾指出一处测试 clone 样式，按建议改用 from_ref 后通过，没有放宽检查。Web 仅同步版本，类型检查和生产构建通过。
- 最终二进制 SHA-256：`f482cfc4c4597e8682e98445656d27c86eb8f78a18cda75c5b5bd515d5c208cb`。
- 用该二进制执行 `native-trial --preview`，真实 Worker 组装首请求后停止，HTTP 与模型请求均为 0。基线首请求 UTF-8 为 5237 字节，加根代理 1024 framing 余量及 4000 输出 cap 后为 10261，小于单尝试 16000 token 上限；后续请求仍需逐次独立预算预留，不能由此推断整次可无限运行。

免费夹具实际验证了只连接本机假代理、正常两请求后通过真实 read_file、opaque 上游路由与批准逻辑模型分开核验、请求 / 回执 / response ID 与真实用量保留、缺用量保持 null、错误批准模型拒绝、无网络预览与非法控制文件拒绝。摘要与辅助请求在传输前被拒绝。

读取通过必须同时具备真实成功事件，以及本题 fixture.txt 的返回内容经同一 tool_call_id 进入下一次有有效回执的模型请求。宿主按实际 offset / limit / max_bytes 重新计算行号结果摘要；相同内容的其他文件、伪造结果摘要及未有效发送的结果均不能通过。该证明与 fixture 前后摘要一起进入机制 verifier 输入。

适配器保留模型原始最终正文及完整 Worker 报告；机制检查不是质量分，质量状态保持 host_only_pending。根控制器统一运行独立 Ruby 评分器，且必须先排除未正常完成、截断、预算耗尽和未知消费。控制文件、capability、账单和本机产物不进入 Git。使用方法见 [原生适配说明](../../AGENT_RSI_NATIVE_TRIAL.md)。
