# q4 Gemma 4 96K 单路完整实测

测试始于 2026-11-04；本报告沿用任务开始日期目录，原始时间均为 UTC。范围：Gemma 4 12B QAT、12B QAT + MTP n-max=2、26B-A4B UD-Q3_K_M；另引用已验证的 Qwen3.6-35B-A3B 吞吐基线，并补跑相同接口和代码题。追加补测真实 Qwen3.8-27B UD-IQ3_XXS 的相同代码与接口，详见 [27B 补测报告](QWEN27.md)。

## 结果与选择依据

**三种 Gemma 配置均完成 96K 单路配置下的 72K 输入测试，各 12 个正式样本，无截断。** 26B Q3 可以在这台 16GB 显存的 q4 上按本次配置运行。它的冷 prefill 约 2,223 tok/s、decode 约 77.6 tok/s；12B + MTP 的冷 decode 约 87.7 tok/s。

**若只按本轮速度和显存选择，12B QAT + MTP 更有优势。** 与 26B Q3 相比，其冷 decode 快 12.95%，但整个冷请求仅缩短 2.18%；64K 前缀复用时两者总耗时基本接近，完整输入重复时 MTP 总耗时少 9.34%。MTP 显存采样比 26B 少约 5,324 MiB（约 5.2 GiB）。这不是整体 coding 能力排名。

全新 72K 输入仍需等待约 33 秒首输出；已有 64K 前缀复用时约 5 秒首输出。仅调整窗口上限不会消除实际长输入的求值成本。

**测试结束后已恢复 Qwen3.6-35B-A3B，实际确认 98,304 上下文、单槽，且本地及公网鉴权模型列表和聊天请求通过。** 没有永久切换常驻模型。

## 具体配置与计时

| 配置 | 权重量化 | 草稿推理 | Jinja | 文件大小 | 请求前后显存采样 |
|---|---|---|---:|---:|---:|
| 既有 Qwen3.6-35B-A3B | UD-Q2_K_XL | 无 | 未启用 | 12,290,628,576 bytes | 12,888–12,940 MiB |
| Gemma 4 12B | 官方 QAT Q4_0 | 无 | 启用 | 6,975,879,296 bytes | 8,088–8,098 MiB |
| Gemma 4 12B + MTP | 官方 QAT Q4_0 | draft-mtp，n-max=2 | 启用 | 主权重同上，assistant 253,708,800 bytes | 8,552–8,562 MiB |
| Gemma 4 26B-A4B | UD-Q3_K_M | 无 | 启用 | 12,728,497,888 bytes | 13,834–13,886 MiB |

硬件：RTX 5070 Ti，16,303 MiB。相同 llama-server 二进制 SHA-256：`31c3055dba9d550d9c8eb7f54d6a1e3a03d16925b434e10f5050dcf3cd34d901`。构建标识为 `b0-unknown`，源码目录名 b10549 不作为已核实的 Git commit。

共同参数：`--ctx-size 98304 --parallel 1 --n-gpu-layers 99 --flash-attn on --cache-type-k q4_0 --cache-type-v q4_0 --batch-size 512 --ubatch-size 128`。正式输入 73,728 tokens，实际输出 512 tokens，temperature 0、seed 42、ignore_eos true，关闭思考。

- “冷”是进程重启就绪后的首个客户端推理请求，输入 KV 命中为 0。临时屏蔽 HTTP 启动预热；llama.cpp 内部加载 warmup 仍开启。不清 OS 页缓存，不重置 GPU 时钟。
- 请求墙钟排除权重加载和服务重启至 ready 的耗时。12B、12B MTP、26B 的重启至 ready 中位数分别约 3.48、4.56、3.48 秒。
- 首输出是客户端收到首个非空 SSE content 的时间，不是严格 GPU TTFT。decode 沿用当前构建的 `(predicted_n - 1) × 1000 / predicted_ms` 口径。
- 各指标分别取三轮中位数，不能把各列相加当作同一次请求的分解。每组三轮不构成统计显著性证明。
- 显存为请求前后采样，不代表持续记录的全程峰值。参数 `--n-gpu-layers 99` 和 GPU 占用不能独立证明全层 GPU 卸载；现有 `load_evidence` 没有完整逐层卸载证据，不据此宣称已验证全层卸载。

## 完整性能表

速率单位 tok/s，时间单位秒。Qwen 吞吐来自此前同机已验证的三轮基线，本次没有重新运行其完整测速矩阵。

| 配置 | 条件 | prefill | decode | 首个非空输出 | 总墙钟 |
|---|---|---:|---:|---:|---:|
| Qwen3.6-35B-A3B | 冷首请求，无 KV | 1,507.21 | 70.01 | 48.929 | 56.202 |
| Qwen3.6-35B-A3B | 热 GPU，无 KV | 1,508.33 | 69.91 | 48.973 | 56.283 |
| Qwen3.6-35B-A3B | 64K 前缀 + 8K | 1,328.51 | 69.55 | 6.179 | 13.532 |
| Qwen3.6-35B-A3B | 完整 72K 重复 | 不作长输入估计 | 69.77 | 0.053 | 7.380 |
| Gemma 4 12B QAT | 冷首请求，无 KV | 2,286.58 | 60.04 | 32.256 | 40.728 |
| Gemma 4 12B QAT | 热 GPU，无 KV | 2,277.36 | 60.37 | 32.432 | 40.923 |
| Gemma 4 12B QAT | 64K 前缀 + 8K | 1,664.11 | 60.01 | 4.934 | 13.459 |
| Gemma 4 12B QAT | 完整 72K 重复 | 不作长输入估计 | 59.71 | 0.098 | 8.666 |
| Gemma 4 12B QAT + MTP | 冷首请求，无 KV | 2,225.46 | 87.65 | 33.141 | 38.962 |
| Gemma 4 12B QAT + MTP | 热 GPU，无 KV | 2,221.94 | 86.49 | 33.243 | 39.069 |
| Gemma 4 12B QAT + MTP | 64K 前缀 + 8K | 1,629.40 | 81.04 | 5.039 | 11.355 |
| Gemma 4 12B QAT + MTP | 完整 72K 重复 | 不作长输入估计 | 86.00 | 0.108 | 6.061 |
| Gemma 4 26B-A4B Q3 | 冷首请求，无 KV | 2,223.08 | 77.61 | 33.175 | 39.830 |
| Gemma 4 26B-A4B Q3 | 热 GPU，无 KV | 2,226.47 | 77.71 | 33.174 | 39.652 |
| Gemma 4 26B-A4B Q3 | 64K 前缀 + 8K | 1,744.90 | 78.02 | 4.705 | 11.263 |
| Gemma 4 26B-A4B Q3 | 完整 72K 重复 | 不作长输入估计 | 77.44 | 0.070 | 6.686 |

26B 相对既有 Qwen 的冷 prefill 提高 47.50%、decode 提高 10.86%、首输出等待减少 32.20%、总墙钟减少 29.13%。64K + 8K 的总墙钟减少 16.77%。

12B 的 MTP 相对同一 12B 基线，冷 decode 提高 46.00%，但总墙钟只缩短 4.34%；64K + 8K、完整重复的总墙钟分别缩短 15.63%、30.06%。MTP 接受率本身不能替代端到端收益。

## 实际缓存与输入一致性

三个正式 Gemma 配置的输入 token 数组 SHA-256 均为 `6276326b2ba89e96aa3f0bfb509100648dbc40da9c6218f8e9597dfed7e6a0c7`，不是只核对了输入长度。Qwen 使用自身模板/tokenizer，其 token 数组与覆盖文本不能视为与 Gemma 完全相同。

| 条件 | Gemma 实际 cache_n | Gemma 实际 prompt_n | Qwen 实际 cache_n / prompt_n |
|---|---:|---:|---:|
| 冷首请求 | 0 | 73,728 | 0 / 73,728 |
| 热 GPU，无 KV | 0 | 73,728 | 0 / 73,728 |
| 64K + 8K | 65,536 | 8,192 | 65,536 / 8,192 |
| 完整重复 | 73,723 | 5 | 73,724 / 4 |

每个正式请求实际输出 512 tokens。命中使用 `timings.cache_n`，新算使用 `timings.prompt_n`，不用顶层 `tokens_cached` 代替命中。完整重复只重算 4–5 tokens，不能用全部 72K 输入除以这段短求值时间来宣传长输入 prefill 吞吐。

12B MTP 共生成 5,832 个草稿 tokens、接受 3,213，合计接受率 55.09%。原生响应明确返回 draft-mtp 并有正的草稿计数。其启动日志的 draft 内存预估警告已保留，随后实际加载、草稿生成与接受均成功。26B 本轮未启用 MTP。

## 接口与代码验收

三个 Gemma 配置各有 14 项检查，均为 13 项通过、1 项失败；吞吐矩阵、部署验证和恢复均通过。既有 Qwen3.6-35B-A3B 接口对照为 13 项中 9 项通过、4 项失败。追加的真实 Qwen3.8-27B 补测为 13 项中 11 项通过、2 项失败；别名检查数量不同，不能直接按总分排名。

三个 Gemma 配置的聊天、旧别名、强制工具调用、带随机回执的工具往返、JSON object 和去重代码题均通过；失败项均为金额题。既有 35B 的工具往返、JSON object 和金额源码响应带 JSON 代码围栏，原严格解析失败；35B 没有启用 `--jinja`，Gemma 与本次 27B 启用了，格式表现包含当前服务模板适配差异。

27B 两题均通过原严格 JSON 解析，去重题全过，金额题漏拒绝 `"1.00\n"`，并在低精度上下文的大金额最终 `quantize` 处抛 `decimal.InvalidOperation`。其强制工具调用通过，但工具结果回传阶段在正文再次输出 add 调用标签，没有返回要求的 result/随机 receipt JSON，工具往返失败。

| 配置 | 去重题，8 个测试方法 | 金额题，9 个测试方法 |
|---|---|---|
| Qwen3.8-27B UD-IQ3_XXS，本次补测 | 8/8 通过 | 7/9 通过；1 failure、1 error；严格 JSON 通过 |
| Qwen3.6-35B-A3B 既有配置 | 6/8 通过；2 errors | 原严格 JSON 解析失败；仅去围栏后的原源码补测为 7/9 通过，2 failures、1 error |
| Gemma 4 12B QAT | 8/8 通过 | 7/9 通过；1 failure、1 error |
| Gemma 4 12B QAT + MTP | 8/8 通过 | 7/9 通过；3 failures、1 error |
| Gemma 4 26B-A4B Q3 | 8/8 通过 | 7/9 通过；2 failures、1 error |

金额题的多个 failure 发生于同一 `test_invalid_rows` 方法内的不同 subTest；另一个方法因异常未通过。因此三种 Gemma 配置都为 7/9 个方法通过，不能直接用 `testsRun - failures - errors` 计算通过方法数。

**26B 的实际失败输入：**

1. 货币 `"USD\n"` 应被拒绝，实际未抛 ValueError。
2. 金额 `"1.00\n"` 同样未被拒绝。两者的正则使用 `match` 与 `$`，允许在末尾换行前匹配。
3. 调用方 `Decimal` 精度为 6，依次 sale `123456789012345678901234567890.12`、sale `0.01`、refund `0.02`，应得 `123456789012345678901234567890.11`，实际在源码第 60 行 `quantize` 抛 `decimal.InvalidOperation`。代码沿用了调用方低精度上下文；异常之后的上下文保持不变断言未执行到。

26B 两份实际源码与原响应、报告中的字节数和 SHA-256 一致；完整审阅未发现读取测试、伪造结果、篡改 unittest、skip 或提前退出。其去重源码 SHA-256 为 `c47967037f1754b4bd579b964681e142fca6e8c087d76032dc5b2e1f9932bf04`，金额源码为 `2c49f4c700bd15e86205d591d989065ede30bf828074b0ca0aceab97a630a25a`。

26B 证据：[验收报告](raw/runs/26b-baseline/checks-20261104T160541Z-20261105-2280303-fmwn77/report.json)、[金额题容器原始结果](raw/runs/26b-baseline/checks-20261104T160541Z-20261105-2280303-fmwn77/programming_decimal_refunds.sandbox.json)。

既有 35B 金额题另做了确定性补测：仅移除原响应的一个 JSON 围栏，源码完全不改，不新增模型请求，原严格 JSON 失败保留。12B 和 35B 的详细失败用例见 [12B 阶段报告](STAGE-12B.md)。真实 27B 的原始成绩、失败输入、工具往返及恢复证据见 [27B 补测报告](QWEN27.md) 和 [27B 机器可读成绩](QWEN27.json)。

代码仅在固定 `python:3.12.12-slim-bookworm` 隔离容器内执行：无网络、只读根文件系统、无凭据挂载、非 root，并限制 CPU、内存和 PID。三种 Gemma 配置使用相同固定测试；所有代码题均无超时或输出截断。同解释器运行的测试器不被泛化为对任意恶意代码的防伪边界；源码审阅结论只针对本轮实际代码。

## 文件、原始证据和恢复

- [四配置机器可读对照](comparison.json)：仅选正式 Gemma retry1/26B 与既有 Qwen 基线，包含相对变化、来源哈希、检查摘要和限制。
- [完整 Gemma 汇总](raw/summary-final.json)：同时保留早期尝试与正式结果。12B 正式比较使用 `12b-baseline-retry1`、`12b-mtp-n2-retry1`，26B 使用 `26b-baseline`；早期尝试不混入本报告统计。
- [26B 原始测速](raw/runs/26b-baseline/results.json)、[部署配置](raw/runs/26b-baseline/deployment.json)、[运行验证](raw/runs/26b-baseline/verification.json)、[整轮执行状态](raw/trial-26b.json)。
- [26B 下载校验](raw/download-26b.json)：95 个分段，最终 12,728,497,888 bytes，SHA-256 `811882f6df81c8ea204a5d382f196d6bd601245091786912d4c70df41cdfa052`。
- [12B 权重校验](raw/download-12b.json)、[MTP 元数据](raw/mtp-metadata.json)、[测试计划](PLAN.md)。
- [Qwen 吞吐基线](../q4-qwen36-moe-96k-2026-11-04/README.md)、[Qwen 金额原源码补测](raw/runs/qwen-coding-extracted/summary.json)。
- [26B 结束后的恢复](raw/restoration-after-26b.json)、[26B 后本地与公网验证](raw/final-verification.json)。
- [真实 27B 同题补测](QWEN27.md)、[27B 成绩 JSON](QWEN27.json)、[27B 结束后的本地与公网验证](raw/final-verification-after-qwen27-20261104T165347Z.json)。

26B 测试于 `2026-11-04T16:12:35Z` 完成恢复；最终验证于 `2026-11-04T16:13:54Z` 全部通过。确认 systemd active、原 Qwen override 哈希与备份一致、正确权重、98,304 上下文、单槽、本地和公网 health HTTP 200、公网鉴权模型列表与聊天 HTTP 200，聊天实际响应模型为 `qwen3.6-35b-a3b`。

追加 27B 补测后，于 `2026-11-04T16:53:12Z` 再次恢复原 35B 配置；`2026-11-04T16:53:47Z` 的 6 项本地及公网检查全部通过。当前恢复身份仍为真实 35B、98,304 上下文与单槽，原 override 哈希一致。

恢复点：`/root/backups/gemma4-trial/20261104T132849Z-qwen36-original/`。恢复命令已实际验证：

```bash
ssh q4-pub ruby /root/benchmarks/gemma4-96k-70043de5/tools/service.rb restore
```

所有模型、完整分段、失败部分分段、服务备份和原始测试证据均保留。下载中断记录分别保存在 `raw/download-26b-interrupted-20261104T154036Z/`、`raw/download-26b-interrupted-20261104T155533Z/`。本次没有数据库迁移，没有修改 WillDeep 应用实现，没有 Git 提交、tag 或推送。

## 适用范围

本轮证明这些具体量化和运行参数能在这台机器上完成所列性能与小型验收测试。仅配置 96K 窗口，正式输入为 72K；未测试填满 96K、96K 代码检索准确率、多路并发、真实仓库修复或 SWE-bench。不同量化、模板配置和模型结构共同影响结果，不能把所有差异单独归因于参数量或 MoE，也不能据两道单次代码题决定整体 coding 排名。
