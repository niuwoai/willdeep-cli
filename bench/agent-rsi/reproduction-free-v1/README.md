# 公共免费复现夹具 v1

这些题是公开的工程链路测试，不是盲测集，不能作为模型能力提升的证据。没有授权模型调用，费用为零。旧 24 题已经公开，后续只能作为回归样本。

`manifest.json` 的 `tasks` 固定六个公开案例、三种缺陷家族，各含缺陷和干净版本：

| case_id | 要求 | 缺陷行 | 公共触发输入 |
|---|---|---|---|
| count-zero-bug / count-zero-clean | 零有效，仅 null/undefined 使用 fallback | bug 第 3 行 | count=0、fallback=7 |
| copy-boundary-bug / copy-boundary-clean | 每个值按顺序复制一次，不多复制 | bug 第 4 行 | values=[2,5] |
| duplicate-first-bug / duplicate-first-clean | 同键第一次值保留，后续忽略 | bug 第 5 行 | x=2、x=9 |

worker 只接收所选 `fixture_file` 的原字节（复制为隔离目录中的 `fixture.txt`）、该案例 `task_prompt` 和共享 `review-output-v2/contract.json` 的 `instructions`。不得把 manifest 中的答案、public_witness、reference 或 host-negative 代码放入 worker。

宿主只运行 manifest 白名单中已核 SHA-256 的 fixture/reference。Ruby 与 Node 均校验白名单；Node `vm` 禁止字符串/wasm 动态代码生成，没有 process 或 require，100ms 执行截止；整个子进程 2 秒、32MiB V8 heap、16KiB 输出上限。模型只能提交有界 JSON 数据，不能提交代码、路径或进程参数。

四字段必须是唯一完整 JSON 对象，不接 Markdown/报告外壳，旧三字段评分器和原付费批次保持原样。数据门禁：UTF-8、最多 16384 bytes、深度最多 8、全部容器的 item 合计最多 128、字符串最多 4096 bytes、非负安全整数（拒绝 -0、float、exponent）、拒重复键和原型保留键。公共题的输入还有更窄 shape/数量限制。

宿主独立执行 fixture 与 reference：defect 必须 claimed actual 等于 fixture 实际输出、claimed expected 等于 reference 实际输出，且两种实际输出不同。未触发 bug 的输入不能通过。clean 的 reproduction=null，复现维度明确 N/A；宿主仍执行全部公开 probes，不能把通过探针理解为证明任意输入都无缺陷。分类/定位使用已冻结公开标签，复现验真来自真实执行，两者分开记录。

host evidence 独立于模型报告，严格四字段 `fixture_sha256`、`read_observed`、`delivered_to_model`、`fixture_unchanged`。缺失、错误 hash、未进入后续模型请求或发生修改均不通过；模型正文不能自己证明真实读取。

```sh
ruby scripts/test_agent_rsi_reproduction.rb
ruby scripts/lib/agent_rsi_reproduction_eval.rb --task count-zero-bug --report /private/tmp/report.json --evidence /private/tmp/evidence.json --output-prefix /private/tmp/reproduction-result
```

CLI 只生成 JSON/Markdown 文件，存在即拒绝覆盖，不执行模型推理或激活候选。`host_negative_fixtures` 只用于宿主超时、输出及沙箱边界测试，不是可提交的 worker 案例。最终免费测试报告保存于 `free-verification.json` / `.md`；这些结果只验证工程链路。
