# 评分器格式修复与免费回放验收

修复版本：0.102.0-rc9；最终解析器：`report-json-v5`。本次只做本地格式识别修复和保存回答回放，新增模型请求与费用均为 **0**。不发布、不安装、不上传、不晋升候选。

## 结果与边界

- 全部 275 份可评分回答均已回放。13 个预算终态和 2 次历史基础设施失败单列，不被重新计为质量样本。
- 格式识别：原始 188 → 事后 267；综合通过：原始 187 → 事后 266。79 份格式误拒恢复识别，所有指标 true → false 均为 0。
- 原始 observation、回答、verifier、manifest、plan、ledger、报告及归档等共 8,924 个目录项，回放前后逐项校验相同。原模型请求仍为 708、累计实扣仍为 $0.667158。
- 仍拒绝 8 份格式：5 份多个非模板完整答案、2 份代码引用反引号嵌套错误、1 份真正不闭合的 JSON。不自动修复或猜选这些答案。另有 1 份格式有效但精确行不正确，分类与行号判定未放宽。
- 结果是事后格式诊断，不替换 [原首轮正式报告](../0.102.0-rc8/paid-evaluation.md)，不能作为新独立质量证据或候选晋升依据，不对 reason 做语义评判。

完整计数、分组与转移见 [JSON](offline-format-replay.json) 和 [Markdown](offline-format-replay.md)。分组保留原 epoch、平台和臂，不把新回放混成原正式成绩。报告不含原始回答、请求 ID、账号或凭据。

回放完成后再次运行原首轮独立最终审计，16,253 项全部通过。三次回放的原目录内容摘要完全相同，保留证明见 [preservation.json](preservation.json)。

## 解析规则和安全回归

规则先形成 [独立文档](../../decisions/2026-10-15-agent-rsi-format-replay-policy.md)，通过合成用例后固定解析器摘要再回放。代码花括号、`${…}`、围栏、字符串转义、行内明确省略示例和纯模板被区别处理；重复键、多个完整答案、代码内嵌答案、嵌套 JSON/Array 容器藏答案、错误类型和真正截断仍拒绝。提取过程不接收 oracle，不选“恰好正确”的片段。

原 v1/v2 评分器和原生程序均保存在原工作区未改。首次 v3 回放恢复 73 份；补齐模板插值及行内裸省略值后 v4 恢复 79 份；随后安全审查收紧嵌套容器歧义检查形成 v5，计数保持 79。各次冻结摘要与此前回放结果分别保存，没有覆盖旧回放。`earlier-replay-01.json` / `earlier-replay-02.json` 是事后调试历史，不是额外模型执行。

免费测试：格式专项 52 项 / 61 断言、回放保护 19 项 / 32 断言、原预算/评分 60 项 / 127 断言、恢复账本 20 项 / 75 断言、已知终态 38 项 / 52 断言，共 **189 项、347 断言通过**。测试分别核对原文/观察/oracle 篡改拒绝、未知费用拒绝、Mac 旧证据兼容、显式 null 拒绝、路径逃逸拒绝及禁止覆盖原目录/旧输出。

## 复跑

工具仅加载纯本地解析器，不加载 Host、Account、Provider 或网络客户端。必须传入一个新的独立输出目录；输出包含逐项新派生 verifier 和原输入内容清单，不会写入 batch 或 frozen-source。对已签名最终审计通过的原始批次，命令形式如下：

```sh
ruby scripts/agent_rsi_format_replay.rb \
  --batch SAVED_BATCH \
  --audit ORIGINAL_PASSING_FINAL_AUDIT_JSON \
  --frozen-source ORIGINAL_FROZEN_HOST_SOURCE \
  --expected-parser-sha256 8594b930064aaf9fb313507acee6ab8b7838b965c705b89d92a25c4e1a3ca463 \
  --output NEW_SEPARATE_OUTPUT_DIRECTORY
```

本次完整逐项派生材料保存在本机独立临时目录 `willdeep-rsi-format-replay-results-20261015-03`，未提交原始回答或账号材料。`preflight-v5.json` 固定执行前的规则、解析器和回放工具摘要及免费测试结果。原始首轮目录与冻结源码保持原位置，不用新解析器覆盖它们。
