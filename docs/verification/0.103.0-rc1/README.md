# P4b 免费部分联合验收

Rust `0.103.0-rc1`、Mac `1.421.0-rc1 / build921` 完成免费工程补齐。Mac 本地提交 `67b0ed1667de180959113e945abb1081c1d0da3f`；两端均未合并、推送、发布或安装，some.im 生产状态未改。

## 完成范围与结果

| 环节 | 实际结果 | 证据 |
|---|---|---|
| 共同输出 | 两端与宿主共用精确四字段 JSON、同字节契约与 12 条原始 JSON 正反例 | [共享契约](../../../bench/agent-rsi/review-output-v2/contract.json) |
| Rust 原生 | 实际 CLI 8 场景、4 参数反例通过；独立免费入口在全局凭据解析前分流 | [原生结果](rust-cli.md)、[JSON](rust-cli.json) |
| Rust 停止 | 2 请求停止 / 4 请求完成、49 token 未准入 / 50 token 已耗尽、超时 / 取消符合预期；在途 usage 保持 null | [资源明细](native-host.json) |
| Mac 原生 | 14 项通过（13 项有断言，1 项为旧无控制文件入口直接返回）；Ruby 门禁 5 项 / 25 断言通过 | [Mac 结果](mac-native.md)、[JSON](mac-native.json) |
| 独立宿主 | Mac 6 个 / Rust 2 个正常案例 8/8 通过，116 项绑定检查，28 次可信夹具 / 参考函数执行 | [联合结果](native-host.md)、[JSON](native-host.json) |
| 资源对照 | 另外 6 个资源场景通过 103 项绑定检查，全部不进入复现 / 质量评分 | [联合结果](native-host.md) |
| 免费评分单测 | 56 项 / 154 断言，含真实 CLI 的 UTF-8 正负例及逐请求提示词证据反例 | [评分器报告](../../../bench/agent-rsi/reproduction-free-v1/free-verification.md) |
| 原预算复算 | 708 个保存请求全部匹配冻结预留规则，未提高任何旧额度 | [预算诊断](budget-audit.md)、[JSON](budget-audit.json) |
| 原件保留 | 8,924 个目录项、原账本、原始分数与冻结程序不变；累计旧费用仍为 $0.667158 | [保留检查](preservation.json) |

原生执行继续保留 `unverified`，宿主客观结果单独保存。干净题复现项为 `not_applicable_clean`，公开探针通过不代表穷尽证明；reason 语义尚未独立评测。本次仅证明工程链路，不支持模型质量改善或候选晋升。

## 构建与回归边界

Rust 共享格式测试 2 项通过；子智能体广泛回归 96 项通过、2 项按原配置忽略。补齐逐请求原文后，最终免费 Worker 5 项重测通过；未变的 CLI 参数解析测试 2 项重测通过。当前前端由本树离线 Yarn 构建，最终 CLI 实际构建成功并执行全新 8 场景。最终二进制 SHA-256 为 `4c73f984d9d3f5ab4d09994f53a29630b06059cd7dd2a1b6872e335ca1287dd9`。

另有预算策略 17 项 / 46 断言、预算历史兼容 10 / 11、原预算与评分门禁 60 / 127、旧格式解析 52 / 61、保存结果回放 19 / 32 全部通过，见 [机器报告](ruby-budget-parser-regression.json)。加上 56 项新复现单测，共 214 项 Ruby 测试、431 个断言通过；Mac 自身的 Ruby 门禁单独计数。

Mac 使用独立缓存实际完成 build-for-testing 与 test-without-building；先前磁盘地板中断的记录保留，后续成功构建与测试的退出码均为 0。空间回收仅涉及本 RSI 独立再生编译缓存及对象，原冻结 App / DD / 源码、已测 Products 与所有原始结果均保留。

## 提示词与证据

两端共享指令 SHA-256 为 `ace7d00ee76f9234df3fe934624ba443402c7dee80255972128199e4ed4c9ae8`，契约原文 SHA-256 为 `91b66fce5a84e96340573e43a0743c5d8205b02d228e9240a4947cc86dad163a`。完整系统提示由各自执行器组装，分别记录。

Rust 原 Worker context 仍绑定其编译前缀。每轮变化的预算上下文意味着实际请求提示不同；新适配器为每个已准入 Mock 请求保留正文与摘要，宿主逐条重算、检查共同前缀。首轮 `request_context_projection` 明确标为派生，不冒充或覆盖原 Worker 事件；未准入请求没有虚构的捕获与用量。初次误把后轮视作首轮的失败验收保留在 `/private/tmp/willdeep-p4b-free-host-interop-initial-01`，最终结果使用新产物目录，不覆盖旧数据。

最终原生原件位于 `/private/tmp/willdeep-p4b-rust-acceptance-final-20261015-fbbea1` 与 `/private/tmp/xedit-p4b-free/build/rsi-free-review-v2`；独立联合报告原件位于 `/private/tmp/willdeep-p4b-free-host-interop-final-01`。仓库仅保存脱敏摘要与公共案例结果，不保存实际提示词正文。

## 下一轮

[新盲测方案](../../decisions/2026-10-15-agent-rsi-next-holdout-plan.md) 仅为元数据提案：旧公开 24 题转回归，未来验证与 holdout 按家族隔离；先聚合重复，再以家族整簇比较。未来题目、答案与随机种子均未生成。模型、实时账号价格、题量、请求 / token / 重试上限及总预算须重新确认，旧 $20 批次授权不沿用。

本阶段新增真实模型请求 **0**，新增评测费用 **0**。操作与实现范围见 [P4b 免费说明](../../AGENT_RSI_P4B_FREE.md)。
