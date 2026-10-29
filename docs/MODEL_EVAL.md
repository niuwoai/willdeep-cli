# 模型行为评测

> 决策记录：`docs/decisions/2026-09-20-experience-baseline-and-model-eval.md` 第 2 项。
> 目的：提示词、路由、上下文压缩的改动要拿数字说话，而不是肉眼看一轮。

## 第一步（已落地）：离线指标

`scripts/session_metrics.rb` 读本机会话记录，只输出计数，不输出任何正文。

```bash
ruby scripts/session_metrics.rb --limit 20 --json /tmp/metrics.json --markdown /tmp/metrics.md
```

| 指标 | 定义 | 为什么看它 |
|---|---|---|
| 人话率 `narration_ratio` | 有正文的 assistant 消息 / 全部 assistant 消息 | 思考型模型把话全写进 reasoning、正文留空，用户整轮看不到一句人话。rc6 之前线上一个会话是 18 / 397 |
| 静默工具轮 `silent_tool_turns` | 只带工具调用、正文为空的 assistant 消息数 | 人话率的另一面，直接对应「几十次工具调用一言不发」 |
| 思维链占比 `reasoning_ratio` | 带 `reasoning` 的 assistant 消息占比 | 判断模型是否在思考模式，解释人话率的成因 |
| 工具调用 / 失败 `tool_calls` / `tool_failures` | 工具调用总数与失败数（失败按结果开头启发式判定） | 失败率抬头往往是提示词或工具描述改坏了 |
| 调用次数 `turns` | assistant 消息数 | 同一任务变长说明模型在绕 |

用法约定：提示词或路由改动前跑一次留底（`--since` 卡改动日期），改动后用同类任务再跑一次，把两份 Markdown 贴进 PR。

## 第二步（rc21 落地）：固定任务集定期跑

### 任务集

`bench/model-eval/tasks/` 里 20 个可自动验收的小任务，每个带 verifier：

| 语言 | fix | feature | test | lint | 小计 |
|---|---:|---:|---:|---:|---:|
| Rust（`cargo test` / `cargo clippy`） | 4 | 3 | 1 | 2 | 10 |
| Ruby（minitest） | 2 | 1 | 1 | 0 | 4 |
| Python（unittest） | 1 | 1 | 1 | 0 | 3 |
| JavaScript（`node --test`） | 1 | 1 | 1 | 0 | 3 |

- **fix**：测试是红的，修实现。**feature**：函数体是 `todo!()`，按文档实现。**lint**：clippy 或 `deny(unused)` 过不了，
  真改而不是 `allow` 掉（`must_not_contain` 卡住）。**test**：实现是对的，补测试；每个任务带两个变异实现，
  新测试抓不住变异就不算过——不然把测试文件写空也能「通过」。
- 验收在模型碰不到的干净目录里做：原始 fixture + 模型改过的 `editable` 文件。改测试、改 Cargo 别名、加 build
  script 都进不了那个目录。受保护文件被动过的记 `cheated`，不进分子。口径细节见 [`bench/model-eval/README.md`](../bench/model-eval/README.md)。
- 任务集自己也要过检：`--check-tasks` 证明每个任务 fixture 原样红、fixture + solution 绿；它不联网，CI 在 Linux 上跑。

### 跑法

```bash
ruby scripts/model_eval.rb --check-tasks                                     # 不联网自检
ruby scripts/model_eval.rb --model glm-5                                     # 一个模型，20 个任务
ruby scripts/model_eval.rb --model glm-5 --model deepseek-v4-flash --kind fix # 多模型、只跑一类
ruby scripts/model_eval.rb --model glm-5 --tasks rust-sum-inclusive --keep   # 留下会话与日志排查
ruby scripts/model_eval_trend.rb --alarm --inject                            # 趋势 + 报警 + 写回本文档
```

每个任务一次 `willdeep run --local --output json --input prompt.md --full-auto --max-turns 24`（墙钟 300 秒），
工作区是 fixture 铺成的临时 git 仓库，`WILLDEEP_HOME` 指向私有临时目录，所以会话不混进 `~/.willdeep/sessions`。
配置从 `~/.willdeep/config.toml` 派生：原文照抄，只砍 `[notifications]`（六十次 webhook）和 `[mcp_servers.*]`
（用不上，白等）；派生文件 0600、跑完即删，凭据不进报告不进日志。行为指标只认 `session_metrics.rb` 这一条算路。

### 报告与归档

`bench/model-eval/reports/<日期>/<模型>.<时刻>.json|md` 存完整报告，`history.jsonl` 每模型每轮一行：

| 字段 | 含义 |
|---|---|
| `pass_rate` | verifier 通过率 = passed / 执行数；skipped、error 不进分母，分母为 0 存 `null` |
| `cheated` / `timeouts` / `false_completions` | 动了受保护文件、超时、声称完成但没过 |
| `narration_ratio` / `reasoning_ratio` | 人话率与思维链占比，按模型调用次数加权 |
| `silent_tool_turns` / `tool_calls` / `tool_failures` | 静默工具轮、工具调用与失败 |
| `tokens` / `seconds` | 有检查点用量的任务之和（缺就 `null`）、执行任务墙钟之和 |
| `by_kind` | fix / feature / test / lint 各自的执行数与通过率 |
| `commit` / `dirty` / `binary_version` | 测的是哪版代码、哪版二进制 |

### 没跑成的一轮（0.84.0-rc3 起）

一个模型的任务**一个都没执行**（全是 `error` / `skipped`）时，这一轮不是成绩：不写报告、不进
`history.jsonl`，脚本退出 1，并在 stderr 打出原因——按退出码归类（1 未分类，多半缺 API Key；3 Provider 出错：
鉴权、模型名、网络或代理；127 找不到二进制），附 willdeep stderr 的最后一行（按 `sk-…`、`Bearer …`、
`api_key=…` 打码、截断到 240 字）。开头连续 3 个任务都是 `error` 就不再往下跑这个模型。工作区 git 铺不起来
（例如全局配置要求签名而签名工具不可用）记成该任务的 `error` 并带上 git 自己的报错，不再把整轮带崩；fixture
仓库的提交关掉了签名与钩子。

2026-09-20 ～ 09-27 夜跑每晚 60 个任务全部 `error`（退出码 3，1～17 秒就结束，模型没开工），却照样往
`history.jsonl` 写了 `executed: 0` 的行；失败分支里 `$LOG。` 又在 launchd 的 C locale 下被 bash 读成未绑定变量，
连失败提示都没打出来。同一时间交互 shell 里单任务冒烟能正常执行，说明是夜跑环境的问题（launchd 没有交互
shell 的环境变量：`api_key_env` 指向的变量、`HTTPS_PROXY` 等），不是脚本把任务跑丢了。排查：

```bash
ruby scripts/model_eval.rb --model glm-5 --tasks js-add-test-clamp --no-archive --keep --max-turns 2
# 看保留的运行目录里 logs/stderr.log；夜跑需要的环境变量写进 plist 的 EnvironmentVariables
```

### 报警

`model_eval_trend.rb --alarm`：每个模型拿最近一轮与基线比，**verifier 通过率或人话率任一掉超过 10 个百分点**
即报警、退出 1。基线取至少七天前的最近一轮；还没跑满一周就拿上一轮凑合；只跑过一轮则没得比、不报警。
`null` 的指标跳过，不当 0 比。

### 定时

不进 PR 的 CI——要真凭据、要网络、每轮都花钱。本机用 launchd 每天 03:30 跑 `scripts/model_eval_nightly.sh`
（模板与安装见 [`scripts/launchd/`](../scripts/launchd/README.md)），模型列表由 `WILLDEEP_EVAL_MODELS` 给，缺省
`glm-5,deepseek-v4-flash,deepseek-v4-pro`；要评 Claude，把它的模型 ID 加进去即可。跑完把 `history.jsonl`、
`reports/`、本文档的趋势区块留在工作区等人 review，不自动提交、不自动 `git pull`；评测没跑成趋势不更新，
跑成了但报警退出 2。稳定后再评估是否上专用 runner。

## 趋势

由 `ruby scripts/model_eval_trend.rb --inject` 生成，别手抄。

<!-- model-eval:begin -->
### glm-5

最近一轮 **2026-10-28T19:45:28Z** · 代码 `fecf90f3`（工作区不干净） · 执行 20/20 · 通过 20 · 作弊 0 · 出错 0

| 指标 | 最近一轮 | 对比基线（2026-10-18） | 趋势 |
|---|---|---|---|
| verifier 通过率 | 100% | ±0 | `██████▁▁██` |
| 人话率 | 77% | +4 | `▇▁▇▃▃▁··▆█` |
| 误报完成率 | 0% | ±0 | `▄▄▄▄▄▄▄▄▄▄` |
| 思维链占比 | 71% | +1 | `▆█▃▇▄▆··▁▃` |

<details><summary>历史 10 轮</summary>

| 时间 | 代码 | 执行 | 通过率 | 作弊 | 人话率 | 静默工具轮 | 工具失败 | 秒/任务 |
|---|---|---:|---|---:|---|---:|---:|---:|
| 2026-10-28T19:45:28Z | `fecf90f3` | 20 | 100% | 0 | 77% | 40 | 0 | 45 |
| 2026-10-18T19:45:17Z | `bd436f0` | 20 | 100% | 0 | 73% | 45 | 0 | 44 |
| 2026-10-18T00:46:52Z | `bd436f0` | 20 | 0% | 0 | - | 0 | 0 | 908 |
| 2026-10-17T01:07:08Z | `bd436f0` | 20 | 0% | 0 | - | 0 | 0 | 919 |
| 2026-10-15T19:46:58Z | `bd436f0` | 20 | 95% | 1 | 64% | 69 | 0 | 49 |
| 2026-10-14T19:44:52Z | `bd436f0` | 20 | 100% | 0 | 68% | 58 | 0 | 43 |
| 2026-10-13T19:48:19Z | `bd436f0` | 20 | 95% | 1 | 67% | 64 | 0 | 50 |
| 2026-10-12T19:45:48Z | `bd436f0` | 20 | 100% | 0 | 74% | 46 | 0 | 45 |
| 2026-10-11T19:47:32Z | `899dbbf` | 20 | 100% | 0 | 64% | 72 | 0 | 51 |
| 2026-10-10T19:46:05Z | `899dbbf` | 20 | 100% | 0 | 74% | 48 | 0 | 47 |

</details>

### deepseek-v4-flash

最近一轮 **2026-10-28T19:54:18Z** · 代码 `fecf90f3`（工作区不干净） · 执行 20/20 · 通过 19 · 作弊 1 · 出错 0

| 指标 | 最近一轮 | 对比基线（2026-10-18） | 趋势 |
|---|---|---|---|
| verifier 通过率 | 95% | -5 | `██████▃▁██` |
| 人话率 | 53% | +0 | `▆▃█▂▃▃▁·▂▂` |
| 误报完成率 | 5% | +5 | `█▁▁▁▁▁▁▁▁█` |
| 思维链占比 | 50% | +5 | `██▇██▇▁·▃▅` |

<details><summary>历史 10 轮</summary>

| 时间 | 代码 | 执行 | 通过率 | 作弊 | 人话率 | 静默工具轮 | 工具失败 | 秒/任务 |
|---|---|---:|---|---:|---|---:|---:|---:|
| 2026-10-28T19:54:18Z | `fecf90f3` | 20 | 95% | 1 | 53% | 98 | 0 | 26 |
| 2026-10-18T19:54:20Z | `bd436f0` | 20 | 100% | 0 | 52% | 105 | 0 | 26 |
| 2026-10-18T05:32:58Z | `bd436f0` | 20 | 0% | 0 | - | 0 | 0 | 856 |
| 2026-10-17T04:52:03Z | `bd436f0` | 20 | 25% | 0 | 52% | 29 | 0 | 625 |
| 2026-10-15T20:00:31Z | `bd436f0` | 20 | 100% | 0 | 54% | 95 | 0 | 39 |
| 2026-10-14T20:00:49Z | `bd436f0` | 20 | 95% | 0 | 54% | 91 | 0 | 47 |
| 2026-10-13T20:02:12Z | `bd436f0` | 19 | 100% | 0 | 53% | 101 | 0 | 40 |
| 2026-10-12T19:59:13Z | `bd436f0` | 20 | 100% | 0 | 61% | 92 | 0 | 38 |
| 2026-10-11T19:57:04Z | `899dbbf` | 20 | 100% | 0 | 54% | 108 | 0 | 28 |
| 2026-10-10T19:55:13Z | `899dbbf` | 20 | 95% | 1 | 58% | 94 | 0 | 26 |

</details>

### deepseek-v4-pro

最近一轮 **2026-10-28T20:04:41Z** · 代码 `fecf90f3`（工作区不干净） · 执行 20/20 · 通过 19 · 作弊 1 · 出错 0

| 指标 | 最近一轮 | 对比基线（2026-10-20） | 趋势 |
|---|---|---|---|
| verifier 通过率 | 95% | -5 | `███▂▁███▁█` |
| 人话率 | 59% | -2 | `▅▄▄▁·▂▇█·▇` |
| 误报完成率 | 5% | +5 | `▁█▁▁▁▁▁▁▁█` |
| 思维链占比 | 45% | +45 | `█▇▇▅·▇▁▁·▇` |

<details><summary>历史 10 轮</summary>

| 时间 | 代码 | 执行 | 通过率 | 作弊 | 人话率 | 静默工具轮 | 工具失败 | 秒/任务 |
|---|---|---:|---|---:|---|---:|---:|---:|
| 2026-10-28T20:04:41Z | `fecf90f3` | 20 | 95% | 1 | 59% | 67 | 0 | 30 |
| 2026-10-25T21:20:14Z | `f63c8fe` | 20 | 0% | 0 | - | 0 | 0 | 327 |
| 2026-10-20T19:41:14Z | `bd436f0` | 20 | 100% | 0 | 61% | 69 | 0 | 32 |
| 2026-10-19T19:41:22Z | `bd436f0` | 20 | 100% | 0 | 60% | 72 | 0 | 33 |
| 2026-10-18T20:02:21Z | `bd436f0` | 20 | 100% | 0 | 53% | 76 | 0 | 23 |
| 2026-10-18T10:40:16Z | `bd436f0` | 20 | 0% | 0 | - | 0 | 0 | 919 |
| 2026-10-17T09:12:27Z | `bd436f0` | 20 | 15% | 0 | 52% | 15 | 0 | 779 |
| 2026-10-15T20:09:42Z | `bd436f0` | 20 | 100% | 0 | 56% | 78 | 0 | 26 |
| 2026-10-14T20:09:46Z | `bd436f0` | 20 | 95% | 1 | 56% | 78 | 0 | 26 |
| 2026-10-13T20:16:19Z | `bd436f0` | 20 | 100% | 0 | 57% | 77 | 0 | 40 |

</details>
<!-- model-eval:end -->
