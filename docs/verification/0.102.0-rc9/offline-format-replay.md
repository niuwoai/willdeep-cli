# 已保存回答的免费格式回放

**事后格式诊断，原始评分全部保留；不是新的付费评测，也不能据此晋升候选。**

解析器：report-json-v5；回放 275 份可评分回答；排除 13 个预算终态与 2 次历史基础设施失败。

新增模型请求：0。原 708 个请求与 667158 微美元实扣保持不变。原评测目录 8924 个目录项逐项校验未改。

| 指标 | 原始计数（不覆盖） | 新解析器免费回放 | false → true | true → false |
|---|---:|---:|---:|---:|
| schema_valid | 188 | 267 | 79 | 0 |
| classification_correct | 188 | 267 | 79 | 0 |
| localization_correct | 187 | 266 | 79 | 0 |
| read_observed | 275 | 275 | 0 | 0 |
| fixture_unchanged | 275 | 275 | 0 | 0 |
| passed | 187 | 266 | 79 | 0 |

## 仍拒绝的格式

- multiple_complete_objects：5。
- incomplete_json_or_unknown_brace：1。
- invalid_json_or_duplicate_key：2。

按原评分 epoch、平台和臂分别汇总于 JSON；不将不同正式协议合成原首轮成绩。分类、精确行、读取和 fixture 门禁保持原样，不评判理由语义或泛化能力。
