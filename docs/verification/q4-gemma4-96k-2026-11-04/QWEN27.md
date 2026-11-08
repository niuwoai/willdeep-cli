# Qwen3.8-27B 同题代码补测结果

**已补测真实的 Qwen3.8-27B UD-IQ3_XXS：去重题 8/8 个测试方法通过，金额题 7/9 通过。** 两份源码均通过原严格 JSON 解析，直接执行模型原始源码，没有去围栏或修改代码。方法通过数与三种 Gemma 配置相同。

本轮使用 q4 保留的原 27B 权重，部署身份记录实际为 `/models/Qwen3.8-27B-UD-IQ3_XXS.gguf`，返回模型名为 `qwen3.8-27b`。此前名为 `qwen-coding-control` 的记录实际是 35B，其 `qwen3.8-27b` 只是兼容别名，不作为 27B 成绩。

## 同题成绩

| 配置 | 去重题：8 个方法 | 金额题：9 个方法 | JSON/运行说明 |
|---|---:|---:|---|
| **Qwen3.8-27B UD-IQ3_XXS，本次补测** | **8/8** | **7/9** | 两题严格 JSON 通过；金额 1 failure、1 error |
| Qwen3.6-35B-A3B UD-Q2_K_XL，既有配置 | 6/8 | 7/9，仅去围栏补测 | 金额原严格 JSON 失败；去围栏后源码不变，2 failures、1 error |
| Gemma 4 12B QAT Q4_0 | 8/8 | 7/9 | 金额 1 failure、1 error |
| Gemma 4 12B QAT Q4_0 + MTP n-max=2 | 8/8 | 7/9 | 金额 3 failures、1 error |
| Gemma 4 26B-A4B UD-Q3_K_M | 8/8 | 7/9 | 金额 2 failures、1 error |

每题每配置只有一次有效源码响应。本次 27B 启用了 `--jinja`，与三种 Gemma 相同；既有 35B 代码对照未启用 Jinja。因此不能将接口格式差异全部归因于模型能力，也不能由这两题推断整体 coding 能力相当。多个 subTest 的 failure 可以属于同一个测试方法，方法通过数不是简单减去 failures 和 errors。

## 27B 金额题的实际失败

1. **金额尾随换行未拒绝。** 输入 `{"currency":"USD","kind":"sale","amount":"1.00\n"}` 应抛 `ValueError`，实际被接受。源码第 4、22 行使用 `^…$` 与 `re.match`，`$` 可以在末尾换行前匹配。
2. **低精度上下文下大金额抛 `decimal.InvalidOperation`。** 测试在 `context.prec = 6` 下依次输入 sale `123456789012345678901234567890.12`、sale `0.01`、refund `0.02`，期望 `123456789012345678901234567890.11`。源码虽在循环内使用 `localcontext()`、精度 50，但循环后的最终 `quantize` 位于该上下文之外，第 34 行重新使用调用方低精度并抛错。该异常之后的上下文保持不变断言未执行到。

原始输出：`Ran 9 tests`，`FAILED (failures=1, errors=1)`。失败方法为 `test_invalid_rows`、`test_large_values_and_decimal_context`，其余 7 个方法通过。

证据：[金额原源码](raw/runs/qwen38-27b-coding-retry1/checks-20261104T165250Z-20261105-2305583-i9gtug/fixture_programming_decimal_refunds/solution.py)、[固定测试与容器输出](raw/runs/qwen38-27b-coding-retry1/checks-20261104T165250Z-20261105-2305583-i9gtug/programming_decimal_refunds.sandbox.json)。

## 接口验收

共 13 项：**11 通过、2 失败、0 未运行**。模型身份、聊天、临时兼容别名、强制 add 工具调用、JSON object、隔离环境和去重题均通过；失败为金额题和工具往返。

**工具往返失败：** 首次强制 `add(19, 23)` 成功；随后已提供工具结果 42 与本轮随机 receipt，并要求只返回相应 JSON，实际正文再次输出 `<tool_call>…add…</tool_call>`，没有按要求返回 JSON。原响应 `finish_reason = stop`，没有结构化 `tool_calls`；验收中的严格 `JSON.parse` 失败。本轮无法据此认定模型端到端工具往返可靠。

证据：[接口与代码验收报告](raw/runs/qwen38-27b-coding-retry1/checks-20261104T165250Z-20261105-2305583-i9gtug/report.json)、[工具往返原响应](raw/runs/qwen38-27b-coding-retry1/checks-20261104T165250Z-20261105-2305583-i9gtug/api/tool_round_trip.response.json)。

## 身份、配置与一致性

- 权重：`Qwen3.8-27B-UD-IQ3_XXS.gguf`，10,934,860,704 bytes；完整 SHA-256 `c0b7c3038681ed2e3040456c1dd45f9858b6c2290bed172c70388a94874f3eee`。该哈希取自 q4 保留的既有权重，用于本次身份一致性检查，未与上游 checksum 比对。
- 相同 llama-server 二进制 SHA-256：`31c3055dba9d550d9c8eb7f54d6a1e3a03d16925b434e10f5050dcf3cd34d901`。
- 上下文 98,304、parallel 1、GPU layers 99、Flash Attention、K/V q4_0、batch 512、ubatch 128。相较旧 27B 配置增加 Jinja、Docker 内存 28g 与 16 CPU 限额；临时兼容别名为 `qwen3.8-27b-compat`。
- 原验收工具 `checks.rb` 完全不改，SHA-256 `125f9a67335256c2864715c73fb8f48ca6380200fc5b8a1aceeba17aea609f48`。代码请求 temperature 0、关闭 thinking、max_tokens 4096；两题均 `finish_reason = stop`。
- 固定去重测试 SHA-256：`7a4fdabf0a0d38ec7be8563375e0bbd398cb091ffe4483cf37cd30c61e853b80`。
- 固定金额测试 SHA-256：`0ed63728cbd21a4f1cac6de2a4666679a71918d1b783b4b2bbb9daefebbfb0e8`。
- 去重原源码 SHA-256：`06dbc9a08686ee456952040d5a410498e97e78e9379de68c0f6106a1deadb3ec`；金额原源码 SHA-256：`39bdd465687401852189a314c33e47883e6e1b91b02c45744089e46490aa157f`。已核对原响应解析出的 source、实际夹具与验收记录一致。
- 代码仅在 `python:3.12.12-slim-bookworm` 隔离 Docker 内执行：无网络、非 root、只读、无凭据，1 CPU、256 MiB、64 PID，Python `-I`。两题均未超时、未截断、无 skip。审阅本轮两份实际源码，未发现读取测试、修改 unittest 或伪造结果。

身份与参数见 [实际部署身份](raw/runs/qwen38-27b-coding-retry1/deployment-identity.json)、[部署清单](raw/runs/qwen38-27b-coding-retry1/deployment.json)、[配置差异](raw/runs/qwen38-27b-coding-retry1/configuration-comparison.json)。机器可读成绩见 [QWEN27.json](QWEN27.json)。本轮没有重跑吞吐矩阵。

## 首轮工具配置失败与有效轮次

首轮 `qwen38-27b-coding` 在部署身份校验时退出，没有执行代码题。原因是工具将 35B 名称加入临时 27B 别名；当前 llama.cpp 用 `std::set` 保存别名并以其第一个元素作为返回名，按字母排序返回了 35B 名称。此轮不计入模型成绩，已保留 [首轮执行状态](raw/runs/qwen38-27b-coding/status.json)。

工具 `run_qwen27.rb` 修为 1.0.3，分开保存恢复名与临时兼容名；在本机和 q4 离线验证通过后，使用全新 `qwen38-27b-coding-retry1` 目录重跑。原子配置替换、写入失败不停止服务等保护仍通过验证，固定题目、测试和模型源码均未修改。

## 35B 恢复与验证

补测于 `2026-11-04T16:53:12Z` 恢复原 Qwen3.6-35B-A3B。随后 `2026-11-04T16:53:47Z` 的 6 项本地及公网检查全部通过：systemd active、原 override SHA-256 匹配、原二进制匹配、实际 35B 权重、98,304 上下文、单槽空闲；本地/公网 health、公网鉴权模型列表及聊天均 HTTP 200，聊天实际返回 `qwen3.6-35b-a3b`。

- [本轮恢复记录](raw/runs/qwen38-27b-coding-retry1/restoration.json)
- [补测后的本地及公网验证](raw/final-verification-after-qwen27-20261104T165347Z.json)
- 原 35B 备份：`/root/backups/gemma4-trial/20261104T132849Z-qwen36-original/`；本轮另存原 unit 与 override 于远端 `runs/qwen38-27b-coding-retry1/`。
- 恢复后 override SHA-256：`0b20cd577c3562f1215c78c56a6e3330e52566f9e990ef6c12b58d2748612b0e`。

若之后再次需要从该已验证恢复点恢复，可使用既有带哈希守卫的恢复工具：

```bash
ssh q4-pub ruby /root/benchmarks/gemma4-96k-70043de5/tools/service.rb restore
```

所有权重、备份与两轮原始证据均保留。无数据库迁移，无 WillDeep 应用实现变更，无 Git 提交、tag 或推送。目录沿用任务开始日期；本报告时间均为 UTC。
