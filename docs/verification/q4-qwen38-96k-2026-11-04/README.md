# q4 Qwen3.8-27B：96K 单路部署与 72K 冷热测速

测试日期：2026-11-04。原始记录时间使用 UTC；完成时间为 10:28:48 UTC（北京时间 18:28:48）。

## 结果

q4 已从 **192K 单路**调整为 **96K 单路**：`--ctx-size 98304 --parallel 1`。进程参数、`/props`、`/slots` 和 `/v1/models` 均已核验一致。服务保持运行，普通 `/v1/chat/completions` 冒烟请求正常返回 `OK`。

每组 3 次，所有正式请求均为 **73,728 tokens 输入、512 tokens 输出**，没有上下文截断。以下为中位数：

| 条件 | 命中输入缓存 | 本次实际 prefill | Prefill（tok/s） | Decode（tok/s） | 首个非空输出（秒） | 请求总耗时（秒） |
|---|---:|---:|---:|---:|---:|---:|
| 重启后首请求，冷 KV | 0 | 73,728 | **1,000.7** | **26.33** | **73.69** | 93.09 |
| 热 GPU，无 KV 复用 | 0 | 73,728 | **1,000.6** | **26.29** | **73.87** | 93.31 |
| 热前缀：64K 已缓存，新增 8K | 65,536 | 8,192 | **732.6** | **26.29** | **11.20** | 30.63 |

三次测量的范围：

| 条件 | Prefill 范围（tok/s） | Decode 范围（tok/s） |
|---|---:|---:|
| 重启后首请求，冷 KV | 999.99–1,007.68 | 26.324–26.392 |
| 热 GPU，无 KV 复用 | 999.92–1,004.27 | 26.284–26.300 |
| 64K 缓存 + 8K 新输入 | 731.70–734.46 | 26.276–26.293 |

另做了 **3 次完整 72K 输入的重复请求**：实际命中 73,724 tokens，末尾重算 4 tokens；首个非空输出中位数 **0.105 秒**，decode **26.27 tok/s**，总耗时 **19.57 秒**。仅处理 4 tokens 的阶段太短，不用它估计长输入的 prefill 吞吐。

### 如何解释

- 72K 全新输入需要约 **73.7 秒**处理提示词，输出阶段约 **26.3 tok/s**。
- 复用 64K 前缀后，首个输出等待降至约 **11.2 秒**。此时 `732.6 tok/s` 衡量新增的末尾 8K；它与从头处理完整 72K 的平均吞吐覆盖不同位置范围，不能当成对同一段输入的直接快慢比较。
- 热前缀按整段 72K 除以 prefill 时间的等效吞吐约 **6,594 tok/s**；这包含免于计算的缓存部分，不能称为真实 prefill 吞吐。
- 缓存显著减少输入等待，输出速度仍约 **26.3 tok/s**。本次载荷输出 512 tokens 约需 **19.4 秒**，三组总耗时差别主要来自输入阶段。
- 本次冷测是“模型 ready 后的无输入 KV 缓存首请求”，不包含加载模型的耗时，也没有清空操作系统文件缓存或将 GPU 断电。三次重启至 ready 的中位耗时另为 **3.75 秒**。
- 测试覆盖 72K 输入和 512 输出，验证了该负载下可运行；未测试填满 96K 边界，也未评测长上下文回答正确率。本轮没有 192K 配置下相同载荷的对照，不能据此归因窗口调整带来了多少性能变化。

## 环境与配置

| 项目 | 值 |
|---|---|
| 主机 | q4，SSH 走 `q4-pub` |
| GPU | NVIDIA GeForce RTX 5070 Ti，16,303 MiB |
| 模型 | Qwen3.8-27B `UD-IQ3_XXS`，API alias `qwen3.8-27b` |
| GGUF 权重参数量 | 27,320,697,856 |
| llama.cpp 源目录 | `/home/ml-data/src/llama.cpp-b10549` |
| 二进制 SHA-256 | `31c3055dba9d550d9c8eb7f54d6a1e3a03d16925b434e10f5050dcf3cd34d901` |
| 服务 build_info | `b0-unknown`，目录不含可读 git commit；以二进制哈希溯源 |
| 容器基础镜像 | `nvidia/cuda:12.9.1-devel-ubuntu24.04` |
| 上下文 / 并发 | 98,304 tokens / 1 |
| GPU offload | `--n-gpu-layers 99` |
| KV 类型 | K=`q4_0`，V=`q4_0` |
| Flash Attention | `on` |
| batch / ubatch | 512 / 128 |
| 观测显存占用 | 12,670–12,892 MiB，约 12.37–12.59 GiB；为请求前后采样，不作为瞬时峰值 |
| 最终状态 | 服务 active/running，槽位空闲，API 上报上下文 98,304 |

模型、量化、GPU offload、batch、端口与原服务一致。持久变更为上下文 196,608 → 98,304，以及对应容器名称和服务描述。并发保持 1。

## 测量方法与口径

1. 使用 llama.cpp 原生 `/completion`，通过 q4 本机 loopback 请求，避免把公网隧道传输算进服务端吞吐。
2. 使用固定的合成 Rust 风格记录，构建 Qwen ChatML 输入，包含明确闭合的 thinking 前缀。通过模型自身 `/tokenize` 构造 token ID 数组，总输入严格为 73,728；包含 45 个头部 tokens 和 33 个尾部 tokens。输入不是用户仓库代码。
3. 请求使用 `n_predict=512`、`ignore_eos=true`、`temperature=0`、`seed=42`，全部生成 token 均计入输出。原生固定输入测试不能代表任意聊天模板和 thinking 配置的速度。
4. 冷 KV 每次重启服务。临时屏蔽已有 `ExecStartPost` 请求预热，确保健康检查之后的第一条推理请求就是 72K 主测试；llama.cpp 自身的加载/内核 warmup 不包含在本次处理时间中。
5. 热 GPU 无缓存组使用不同短前缀替换已有缓存，再以 `cache_prompt=false` 请求完整 72K。热前缀组先建立相同输入的前 65,536 tokens，只生成 1 token，再请求完整 73,728 tokens。
6. **实际命中数使用 `timings.cache_n`**，实际求值数使用 `timings.prompt_n`。每个样本均校验二者之和等于 73,728。该版本顶层 `tokens_cached` 是生成后驻留槽位的 token 数，包含生成的历史，不能当成输入缓存命中数。
7. Prefill 和 decode 直接采用服务端 `timings.prompt_per_second`、`timings.predicted_per_second`。该版本 decode 速率按 `(predicted_n - 1) / predicted_ms` 计算；首个生成 token 的求值包含在 prefill 阶段。
8. 首个输出延迟指客户端收到首个非空内容块的墙钟时间；它不等于 GPU 层面精确的首 token 计算时间。三个核心条件串行，各测 3 次；完整输入重复条件另测 3 次。
9. 测试结束已删除临时 systemd drop-in，恢复原有 `/usr/local/sbin/qwen38-warmup` 启动预热配置；持久 96K 配置保留。

输入 token 数组 SHA-256：`d7885766c663b1819d8d1eda8a074164465ca33018ad23c880d57e2c0bc876a4`。

## 测试过程中的恢复记录

九个核心样本全部完成后，额外的整段重复检查因脚本错误假定“最多重算末尾 2 tokens”而退出。实际该构建重算 4 tokens；校验已改为核实真实缓存数、求值数和总输入的一致性，并补齐 3 次整段重复样本。九个核心样本保持原始数据，未用重新测试覆盖。

补测结束后 SSH 回传连接出现超时，但 q4 的持久结果已于 **10:28:48 UTC** 写入 `status=completed`，包含全部 **12 个正式样本和 3 个前缀预热样本**。已取回并校验该结果，且独立执行最终服务检查与聊天接口冒烟验证。

## 备份与回滚

调整前原配置备份在 q4：

`/root/backups/qwen38-27b/20261104T100426Z-before-96k/`

包含 `service.original`、`override.conf`、`manifest.json`。测试脚本、精确 token 输入、原始结果及部署清单保存在：

`/root/benchmarks/qwen38-96k-70043de5/`

如需恢复此前的 **192K 单路**，执行以下命令。回滚脚本会先确认当前仍是此次 96K 配置，再恢复原 drop-in、重载 systemd 并重启服务：

```bash
ssh q4-pub ruby /root/benchmarks/qwen38-96k-70043de5/rollback.rb
```

本次未执行回滚，当前服务保持 96K。

## 原始数据

- [原始测试结果与恢复记录](raw/results.json)
- [样本校验与统计汇总](raw/summary.json)
- [最终服务配置与 API 核验](raw/verification.json)
- [部署和备份清单](raw/deployment.json)

原始结果 SHA-256：`af06b72937f8f2fd06cc36a7db5c82482c032232c63ebb37fda10b92ac513f5c`。

本次仅调整 q4 模型服务并记录验证结果，没有修改 WillDeep 应用代码，也没有提交 Git 变更。
