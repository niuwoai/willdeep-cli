# q4 Gemma 4 12B QAT 阶段实测

本文件保留 12B 阶段截点；26B 已完成，最终比较与服务恢复证据见[完整实测报告](README.md)。

日期：2026-11-04。阶段范围：12B QAT 基线、12B 配套 MTP n-max=2，以及既有 Qwen3.6-35B-A3B 的接口与两项代码题对照。**26B-A4B Q3 在本阶段尚未实测，不能据此完成最终选型。**

## 主要结果

- **12B QAT 基线**：72K 冷输入 prefill **2,286.58 tok/s**、decode **60.04 tok/s**，首个非空输出 **32.256 秒**。
- **12B QAT + MTP n2**：同条件 prefill **2,225.46 tok/s**、decode **87.65 tok/s**，首个非空输出 **33.141 秒**。
- 相对同一 12B 基线，MTP 冷 decode 提高 **46.00%**，但冷请求总墙钟仅从 **40.728 秒**缩短到 **38.962 秒**，减少 **4.34%**。长输入的主要等待仍来自 prefill。
- 相对既有 Qwen 冷请求记录，12B MTP 的 prefill 提高 **47.65%**，decode 提高 **25.21%**，首输出从 **48.929 秒**降至 **33.141 秒**，总墙钟减少 **30.68%**。
- 12B 两种配置的去重题均通过，金额题均未通过完整验收。两项单次小题不能代表复杂仓库 coding 能力。

本阶段正式数据采用独立的 `retry1` 目录；早先采集脚本编码故障的原始记录保留，但不混入性能统计。两种配置各有 12 个正式样本，均 HTTP 200、输入 73,728 tokens、实际输出 512 tokens、无截断。三轮同条件的实际缓存计数一致。

## 环境、配置与计时口径

| 项目 | 设置 |
|---|---|
| GPU | NVIDIA GeForce RTX 5070 Ti，16,303 MiB |
| 引擎 | 现有 llama-server，二进制 SHA-256 见部署记录；源目录名 b10549 不作为已核实的 Git commit |
| 上下文与并发 | 98,304 tokens、单槽 |
| 共同参数 | GPU layers 99、Flash Attention on、K/V q4_0、batch 512、ubatch 128 |
| 请求 | 模型自身模板/tokenizer；73,728 个原生 token ID；temperature 0、seed 42、ignore_eos true；512 实际输出 tokens |
| 模板 | Gemma 启用 `--jinja`、关闭思考；既有 Qwen 服务未启用 `--jinja` |
| MTP | `draft-mtp`、n-max=2、CUDA0、draft K/V q4_0 |

MTP 参数的直接证据：[override.conf](raw/runs/12b-mtp-n2-retry1/override.conf)。两种 12B 配置的实际输入 token 数组 SHA-256 相同：`6276326b2ba89e96aa3f0bfb509100648dbc40da9c6218f8e9597dfed7e6a0c7`。跨模型使用各自 tokenizer，不能要求 Qwen 的 token ID 或对应字符覆盖与 Gemma 相同。

“冷”是重启就绪后的首个客户端推理请求，输入 KV 命中为 0。临时屏蔽 HTTP 启动预热；llama.cpp 内部加载 warmup 仍开启。不清 OS 页缓存，不重置 GPU 时钟，**请求墙钟不包含权重加载和服务启动**。基线/MTP 重启至 ready 中位数分别约 3.48/4.56 秒，另行记录。

首输出是客户端收到首个非空 SSE content 的时间，不是严格 GPU TTFT。prefill 使用服务端实际求值 tokens 和计时；decode 沿用当前构建的 `(predicted_n - 1) × 1000 / predicted_ms` 口径。总墙钟包括完整请求与客户端采集开销。各指标分别取三轮中位数，不保证各列来自同一请求。

## 四种条件的三轮中位数

速率单位 tok/s，时间单位秒。

| 配置 | 条件 | 实际 prefill | decode | 首个非空输出 | 总墙钟 |
|---|---|---:|---:|---:|---:|
| 12B QAT | 冷首请求，无输入 KV | 2,286.58 | 60.04 | 32.256 | 40.728 |
| 12B QAT | 热 GPU，无输入 KV | 2,277.36 | 60.37 | 32.432 | 40.923 |
| 12B QAT | 64K 前缀 + 8K 后缀 | 1,664.11 | 60.01 | 4.934 | 13.459 |
| 12B QAT | 完整 72K 输入重复 | 不作长输入估计 | 59.71 | 0.098 | 8.666 |
| 12B QAT + MTP n2 | 冷首请求，无输入 KV | 2,225.46 | 87.65 | 33.141 | 38.962 |
| 12B QAT + MTP n2 | 热 GPU，无输入 KV | 2,221.94 | 86.49 | 33.243 | 39.069 |
| 12B QAT + MTP n2 | 64K 前缀 + 8K 后缀 | 1,629.40 | 81.04 | 5.039 | 11.355 |
| 12B QAT + MTP n2 | 完整 72K 输入重复 | 不作长输入估计 | 86.00 | 0.108 | 6.061 |

MTP 对四组总墙钟的缩短分别是 **4.34%、4.53%、15.63%、30.06%**。decode 加速并不等于整个长请求按相同比例加速；无 KV 的首输出反而略晚。

| 条件 | 实际 `timings.cache_n` | 实际 `timings.prompt_n` | 实际输出 |
|---|---:|---:|---:|
| 冷首请求 | 0 | 73,728 | 512 |
| 热 GPU、无 KV | 0 | 73,728 | 512 |
| 64K 前缀 + 8K | 65,536 | 8,192 | 512 |
| 完整输入重复 | 73,723 | 5 | 512 |

完整输入重复仍重算 5 tokens，不能把 73,728 除以这段极短求值时间，当成长输入 prefill 算力。命中计数来自 `timings.cache_n`；顶层 `tokens_cached` 不代表输入命中。

原始数据与统计：[12B 汇总 JSON](raw/summary-12b.json)、[基线 results](raw/runs/12b-baseline-retry1/results.json)、[MTP results](raw/runs/12b-mtp-n2-retry1/results.json)。

## 显存与 MTP 实际运行证据

请求前后观测显存：基线 **8,088–8,098 MiB**，MTP **8,552–8,562 MiB**，增加约 **464 MiB**。这是采样区间，不是持续采集得到的全程峰值。既有 Qwen 的相同观测区间为 12,888–12,940 MiB。

| MTP 条件 | 每请求草稿 tokens | 每请求接受 tokens | 接受率 |
|---|---:|---:|---:|
| 冷首请求 | 478 | 272 | 56.90% |
| 热 GPU、无 KV | 478 | 272 | 56.90% |
| 64K + 8K | 507 | 257 | 50.69% |
| 完整重复 | 481 | 270 | 56.13% |

同条件三轮计数相同，12 个正式请求合计生成 5,832、接受 3,213，接受率 **55.09%**。原生响应返回 `none,draft-mtp` 且草稿计数为正，证明草稿路径实际运行。

启动日志保留了一条 draft 内存估算失败警告；随后模型成功加载并完成实际草稿生成和接受。不能将该预估警告解释为 MTP 没有运行。[验证与日志摘要](raw/runs/12b-mtp-n2-retry1/verification.json)。

配套 assistant GGUF 实际含 26 个 F32、23 个 Q4_0 张量，不能仅从文件类型元数据将其称为全 BF16。assistant 通过共享 KV 机制工作，本测试未启用 `--swa-full`。[GGUF 元数据](raw/mtp-metadata.json)。

## 接口与两项代码题

两组 Gemma 均为 14 项检查中 13 项通过、1 项失败，失败项均为金额题。聊天、兼容别名、强制工具调用、带随机 receipt 的工具往返和 JSON object 均通过。Qwen 为 13 项中 9 项通过、4 项失败；别名检查数量不同，不能直接把检查总分当模型排名。

| 配置 | 去重题，8 个测试方法 | 金额题，9 个测试方法 |
|---|---|---|
| 12B QAT | 8/8 通过 | 7/9 通过；1 failure、1 error |
| 12B QAT + MTP | 8/8 通过 | 7/9 通过；3 failures、1 error |
| Qwen 既有配置 | 6/8 通过；2 errors | 原严格 JSON 验收在解析阶段失败；去围栏后的原源码补测为 7/9 方法通过、2 failures、1 error |

Gemma 基线错误接受 `"1.00\n"`。MTP 还接受了 `"01.00"` 与 `"1.٢"`。三条 MTP failure 都属于同一个 `test_invalid_rows` 方法内的 subTest；不能用 `testsRun - failures - errors` 算成 5/9。两组都在调用方低 Decimal 精度下处理大金额时触发 `InvalidOperation`。

Qwen 去重源码额外拒绝合法的空字符串、0、负整数 ID，违背题目约束。金额题原响应带 JSON 代码围栏，严格解析失败；补测仅去除这个围栏，**没有修改源码，没有新增模型请求，原格式失败结论保留**。补测发现非法货币 `ÜSD`、尾随换行金额未被拒绝，以及同样的低精度大金额异常。两条 failure 同属一个方法的 subTest，因此也是 7/9 方法通过。

**Qwen 没有 `--jinja`、Gemma 有，接口格式失败包含当前模板适配差异，不能全部归因于模型本身。** Qwen 工具往返围栏内的计算结果及随机回执正确，但原严格 JSON 检查仍失败。

模型代码仅在固定 Python 镜像的隔离容器内执行：无网络、只读根文件系统、无凭据挂载、非 root、限制 CPU/内存/PID。已审阅实际源码，未发现读取测试、篡改 unittest、伪造结果或跳过测试。此判断针对本轮实际源码，不将同解释器测试器泛化为对任意恶意代码的防伪边界。

代码证据：

- [Gemma 基线验收](raw/runs/12b-baseline-retry1/checks-20261104T151408Z-20261104-2250596-7beorm/report.json)
- [Gemma MTP 验收](raw/runs/12b-mtp-n2-retry1/checks-20261104T152141Z-20261104-2255924-2oy6l7/report.json)
- [Qwen 原严格验收](raw/runs/qwen-coding-control/checks-20261104T153120Z-20261104-2262334-kteg5k/report.json)
- [Qwen 金额补测](raw/runs/qwen-coding-extracted/summary.json)及[原始容器结果](raw/runs/qwen-coding-extracted/decimal_refunds.sandbox.json)

## 既有 Qwen 吞吐对照

本阶段未重跑 Qwen 的完整吞吐矩阵，引用同机、同窗口及相同输入/输出 token 数量的[既有基线](../q4-qwen36-moe-96k-2026-11-04/README.md)。

| 条件 | prefill tok/s | decode tok/s | 首输出秒 | 总墙钟秒 |
|---|---:|---:|---:|---:|
| 冷首请求 | 1,507.21 | 70.01 | 48.929 | 56.202 |
| 热 GPU、无 KV | 1,508.33 | 69.91 | 48.973 | 56.283 |
| 64K + 8K | 1,328.51 | 69.55 | 6.179 | 13.532 |
| 完整输入重复 | 不作长输入估计 | 69.77 | 0.053 | 7.380 |

Qwen 完整重复重算 4 tokens；Gemma 重算 5 tokens。上述为不同模型及量化的整套配置比较，不将差异全部归因于参数量、MoE 或 MTP。

## 服务恢复与范围限制

12B 测试结束后，已实际恢复 Qwen3.6-35B-A3B，验证模型身份、98,304 上下文及单槽。[恢复记录](raw/restoration.json)。后续 26B 临时试验仍需按相同流程恢复和复核；阶段报告不代表此后实时服务状态。

恢复点：`/root/backups/gemma4-trial/20261104T132849Z-qwen36-original/`。恢复命令已实际执行成功：

```bash
ssh q4-pub ruby /root/benchmarks/gemma4-96k-70043de5/tools/service.rb restore
```

本轮没有数据库迁移，没有修改 WillDeep 应用实现，没有 Git 提交、tag 或推送。所有模型、完成分段、失败部分分段和服务备份均保留。

仅配置了 96K 窗口，正式输入为 72K；未测试填满 96K、长文检索正确率、并发、真实仓库修复或 SWE-bench。每组三轮不足以支持统计显著性结论；两道单次作答题也不足以证明哪款模型整体更适合 coding。26B 完成相同口径验证后再给完整比较。
