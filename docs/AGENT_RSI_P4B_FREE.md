# P4b 免费工程补齐

本阶段仅补齐输出、预算停止、最小复现与下一轮盲测准备。全部新执行使用确定性本地 Mock 和本地函数验证，不加载账号或真实 Provider，不复用旧付费授权，不上传 some.im，不激活、同步或晋升候选。

## 共同输出规范

Rust 和 Mac 读取同字节 `bench/agent-rsi/review-output-v2/contract.json` 与 `golden.json`。模型输出只能是一个 JSON 对象，精确包含 `has_defect`、`line`、`reason`、`reproduction`。缺陷必须提供具体输入、需求期望值和夹具实际值；干净判断的行号与复现必须为 null。原始 JSON、重复字段、整数词法及资源边界在两端与宿主独立校验。

报告上限 16 KiB，根深度为 1、最大深度 8，字符串与对象键 UTF-8 不超过 4096 字节；集合成员全树合计最多 128（对象每个键值成员算 1、数组每个元素算 1）。数字仅允许规范非负安全整数；拒绝重复键、多对象、Markdown 围栏、浮点、指数、负零、原型保留键与额外字段。

只在新的 `offline_mock_only` 入口使用这个格式。旧 `native-trial-control.v1`、正常聊天及原评测结果保留原行为；基线定义摘要保持 `51b79a923ceceeae80a7cc6e20f2f0dbdfdb136f1bc516b895450ea633273bdd`。新运行的最终提示、契约、工具及产物有单独摘要，不能与旧实验当成同一配置。

Rust 的原 Worker context 记录组装前缀的提示摘要，Agent 在每轮还会追加变化的运行上下文。新免费观察同时保留原 Worker context、每个已准入 Mock 请求的实际系统提示原文及摘要，分别标明出处；宿主按每轮原文独立重算，不能把后续请求假设为首轮提示。首请求的派生投影不能替换或冒充原生 Worker 事件。两端共享任务与输出指令，整段系统提示仍由各自执行器组装并独立留证，不要求整段逐字相同。这次补充只针对新免费适配器；原付费宿主已保留逐请求原始 JSON，不改写原数据。

## 免费执行与独立复现

Rust 新命令（输出目录必须不存在）：

```sh
willdeep agents rsi free-review --offline-mock-only --output-directory /private/tmp/reviewer-free-bug --fixture bug
willdeep agents rsi free-review --offline-mock-only --output-directory /private/tmp/reviewer-free-clean --fixture clean
```

该入口不接受 Provider 或凭据。确定性 Mock 先要求正常 `read_file`，再从实际收到的工具内容构造公开答案。`raw-report.json`、`evidence.json`、`effective-prompt.txt`、`observation.json` 单独保存；原生结果仍为 `unverified`。模拟 token 是测试数据，不能视为实际模型消费。

Mac 对应 `AgentRSIFreeReview` 与专用本地 URLProtocol，沿正常 Worker / 文件工具路径执行，使用相同任务提示与输出指令。两端均不把 manifest 中的标签、参考函数或 oracle 作为 Worker 输入。当前适配器用于输出与停止机制的工程验收，不是模型能力测试。

统一宿主评分入口需本机 Ruby 与 Node：

```sh
ruby scripts/lib/agent_rsi_reproduction_eval.rb --task count-zero-bug --report /private/tmp/reviewer-free-bug/raw-report.json --evidence /private/tmp/reviewer-free-bug/evidence.json --output-prefix /private/tmp/reviewer-free-bug-verdict
```

评分器只接受有界数据，不执行模型生成的代码、命令、路径或依赖。宿主用固定 manifest 与 SHA 白名单选择本地夹具及参考函数，执行输入后核对实际值、期望值和差异；Node 内执行时限 100 ms，子进程 2 秒、堆与输出有界。文件摘要不一致、伪造实际值、缺少真实读取证据或没有产生差异均不算复现通过。这里的 VM 用来执行固定可信测试函数，不是任意模型代码的安全沙箱。

六个公共案例涵盖三组缺陷 / 干净对照；已公开，不能作下一轮盲测。干净题仅验证公开探针并标记 `not_applicable_clean`，不宣称穷尽证明。`reason` 的语义质量尚未独立评测。分类、精确行、格式、读取证据、复现分别报告，不把缺项算通过。

## Rust 预算预留与停止

`scripts/lib/agent_rsi_budget_policy.rb` 只是纯本地诊断模块，不是新的付费准入器。它复算原规则：序列化 messages / tools / tool_choice 的 UTF-8 字节数加 1024，作为输入保守上界；输出 cap 受 4000、单 trial 16000 与全局 5760000 token 剩余额度共同约束。当前不以分词估算替换上界，不扩大 4 次请求或原 token 限制。

708 个保存请求逐笔一致。Rust 369 次、43 次输出 cap 被压低；Mac 339 次、8 次被压低。输入预留 / 实际输入的中位数分别约 4.295 / 4.285。Rust 12 个预算终态实际消耗为 4250—8384 token，其中 10 个有额外本地拒绝回执；Mac 1 个预算终态为 8631 token。实际消费尚未满额不等于下一次保守预留仍可准入。

本地未准入拒绝没有新增物理消费，不构造虚假的未知账单；已发出的请求缺用量或未对账仍必须保持未知并停止。超时 / 取消若发生在调用途中，模拟回执也保留 usage=null，不冒充零消费或完成。未知状态不得自动重试或借新实验 ID 清零。

只读复算原保存批次：

```sh
ruby scripts/agent_rsi_budget_audit.rb --batch ORIGINAL_BATCH --audit PASSING_FINAL_AUDIT --output NEW_DIAGNOSTIC_DIRECTORY
```

要求通过的原最终审计、manifest / 账本绑定及新输出目录。首个历史定价探针在旧账本没有 response hash，仅沿原审计核对独立身份、路由、用量与单独账单回执，并在新报告明确注明这一证据边界；其余响应必须匹配原摘要，不能套用缺 hash 豁免。分析前后核对原目录全清单。累计旧费用仍为 667158 微美元，新费用为零。

## 回归及后续门禁

```sh
ruby scripts/test_agent_rsi_budget_policy.rb
ruby scripts/test_agent_rsi_budget_audit.rb
ruby scripts/test_agent_rsi_reproduction_report.rb
ruby scripts/test_agent_rsi_grader.rb
ruby scripts/test_agent_rsi_format_replay.rb
```

新盲测仅保存 [元数据方案](decisions/2026-10-15-agent-rsi-next-holdout-plan.md)。未来新题、oracle 和 seed 本阶段均不生成；旧 24 题转回归，按家族隔离新验证与一次性 holdout。两臂和资源先冻结，再由独立保管者生成与审计新题。后续模型、账号实际价格、题量、请求 / 重试 / token 上限、最大费用与原生产物必须再次交用户批准。

原始基线、原评分、免费格式回放结果和付费账本全部保留。本阶段无质量提升或晋升结论，未发布、安装或改动 some.im 生产状态。
