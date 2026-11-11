# reviewer：内置代码审查协议

> 2026-11-11 · 0.148.0-rc1

## 背景

内置 `reviewer` 原先只有一句提示词：「Audit the proposed behavior and evidence, identify concrete correctness or safety risks」。实际派工时，子 Agent 经常给出「可能存在风险」「建议补充测试」这类无法行动的结论；父 Agent 还得自己回到代码里找证据。工具面已经是只读的（search / grep / list / read / git status / git diff），缺的是审查方法，不是权限。

## 决策

### 1. 固定五类审查维度

按优先级检查：正确性、安全、数据与兼容、并发与生命周期、测试与证据。每一类都要求落到具体代码路径，而不是泛泛而谈。先看 diff，再读改动行所依赖的调用方；忽略调用方的「发现」不算发现。

### 2. 每条结论必须可复现

一条发现必须包含：严重级别（blocker / major / minor）、`file:line`、触发条件（输入、状态、顺序）、错误结果。读代码证明路径的标 `CONFIRMED`，缺一块无法证明的标 `PLAUSIBLE`。没有问题就明确写 `No findings` 并说明检查了什么。

### 3. 不报风格问题、不写补丁、不声称验证

reviewer 只判断行为与证据。纯风格、命名、格式问题不报，除非它们造成行为缺陷；补丁由父 Agent 决定。它没有 Shell，因此不得声称跑过命令或测试——这一点写进提示词，避免把「读起来像验证过」当成证据。

### 4. 工具面仍然只读

保留 search / grep / list / read / git status / git diff，新增只读的 `git log`，用于确认改动所处的提交上下文。不加 Shell、不加写工具：子 Agent 无人值守，没有审批卡，权限只能在派工时确定，不能靠运行时兜底。

### 5. 报告契约不变

仍走运行时的 `CONCLUSION / EVIDENCE / OPEN QUESTIONS` 三段式；结构化工作流会用自己的 JSON 契约覆盖它。reviewer 的结论只提供证据，是否阻断、如何修复由父 Agent 决定。

## 边界

- reviewer 不能运行测试或命令，验证性结论只能来自父 Agent 的 verifier 或人类；它能在提示词里说明这一限制，但不能替父 Agent 完成验证。
- 五职责是跨端稳定契约（与 Xedit 对齐），因此不新增第六个「code_reviewer」工种；增强发生在 `reviewer` 内部，`judge` / `security_guard` 的旧名继续路由到它。
- 提示词使用英文，与其余工种一致；面向用户的说明放在本文与 `docs/SUBAGENTS.md`。
