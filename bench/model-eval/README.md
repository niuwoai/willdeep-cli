# 模型行为评测归档

固定任务集上各模型的历次成绩。**这个目录是 git 跟踪的，而且必须是**——理由与
隔壁 `skill-worker-range/` 一样：一次快照能回答「这个模型行不行」，回答不了
「我这次改提示词让它变好了还是变坏了」，而后者才是把评测常态化的全部理由。

## 目录

| 路径 | 是什么 |
|---|---|
| `tasks/<id>/` | 23 个任务（含 3 项 P0 编码回归）。每个带 `task.json` 说明书、`prompt.md`、`fixture/` 起始工作区、`solution/` 参考答案、`mutants/`（kind=test 才有） |
| `history.jsonl` | 每个模型每轮一行摘要，append-only。趋势与报警读的就是它 |
| `reports/<日期>/<模型>.<时刻>.json` / `.md` | 那一轮的完整报告（逐任务状态与行为指标）＋同一份摘要 |

模型正文、会话文件、凭据**不进这里**：它们只在私有临时目录里活到那轮结束
（排查时用 `--keep` 留住）。报告里只有计数与状态。

## 口径

- **passed** = 外部 verifier 通过 **且** 受保护文件逐字未动 **且**（kind=test）每个变异都被新测试抓住。
  验收在模型碰不到的干净目录里重建：原始 fixture + 模型改过的 editable 文件。改测试、
  改 Cargo 别名、加 build script 都进不了那个目录，绿不了。
- **cheated** = verifier 绿了但动了受保护文件。进 `cheated`，不进分子。
- **skipped**（缺可执行文件没跑）与 **error**（宿主 / Provider 出错没让模型开工）**不进分母**。
  分母为 0 时比率存 `null`，不存 `0`——「什么都没跑」和「什么都没过」是两件事。
- **人话率**按模型调用次数加权：60 轮的任务和 3 轮的任务不该各占一票。
- `commit` 是摘要里最重要的字段：没有它，一行成绩就是无主的。工作区不干净时脚本会警告。

## 怎么跑

```bash
ruby scripts/model_eval.rb --check-tasks                      # 不联网：每个任务 fixture 红、solution 绿
ruby scripts/model_eval.rb --model glm-5                      # 真花钱：23 个任务各跑一次 willdeep run
ruby scripts/model_eval_trend.rb --alarm --inject             # 趋势写回 docs/MODEL_EVAL.md，掉 10 个点退出 1
```

`--check-tasks` 进 CI（Linux），它保证任务集本身没坏；真跑不进 CI，定时跑法见
[`docs/MODEL_EVAL.md`](../../docs/MODEL_EVAL.md) 与 [`scripts/launchd/`](../../scripts/launchd/README.md)。

## 分组（split）

每个任务的 `task.json` 都有 `split`，供提示词对照评测（`scripts/prompt_rsi_eval.rb`，设计文档 §8.4）使用：

| 分组 | 数量 | 挑选规则 |
|---|---|---|
| `regression` | 4 | 每种 kind 至少一个的基础题（fix ×2、test、lint）：候选必须全过 |
| `holdout` | 4 | 四种语言各一个，覆盖 feature、fix、test：只在晋升前跑，逐题结果不给任何人看 |
| `train` | 4 | 允许候选生成阅读失败摘要 |
| `validation` | 11 | 其余及 3 项 P0 编码回归，用于选候选 |

- `--split validation,regression` 只跑指定的分组。
- 调整分组要同时改 `scripts/test/model_eval_test.rb` 里的计数断言。分组一改，历史对照结果就不可比了（报告里记有数据集哈希）。

## P0 编码回归

`p0-rust-cross-file-references`、`p0-typescript-symbol-key-references` 和 `p0-protected-multifile-config` 覆盖跨文件 API 引用、字符串键与同名符号边界、受保护配置下的联动修复。Rust 需要 `rustc >= 1.56`（Rust 2021），TypeScript 需要 `Node >= 22.18.0` 的原生类型擦除；这是固定形态与行为验收，不包含完整 TypeScript 类型检查。

```bash
ruby scripts/test/p0_eval_tasks_test.rb --report-dir target/p0-eval-contracts
```

专用离线自检验证原始夹具红、参考答案绿，以及遗漏文件、改受保护文件、硬编码、全局替换等反例；复用现有 verifier 与 JSON / Markdown 报告，报告标明为夹具断言，不写入正式模型成绩历史。LSP 传输和编辑协议的版本、审批、并发与恢复另由 Rust 工具测试覆盖，不能从最终文件通过推导这些协议已经验证。

## 六类核心场景固定回归

[`core-regression.json`](core-regression.json) 登记陈旧编辑、跨文件引用、真实网页三端交互、压缩后约束、多任务恢复与错误完成六类场景。入口复用现有进程监督、独立 verifier 和报告模型，以当前版本 Cargo 产物、SHA-256、精确测试名及非零执行结果核对本轮；`--check-manifest` 仅检查登记，不能签发通过结论。

```bash
ruby scripts/agent_reliability_regression.rb --check-manifest
```

完整基线需要显式 `--with-browser`，并通过重复 `--browser-arg` 传入真实二进制、Node、Chrome 和已启动私有夹具的 manifest。完整参数示例在清单中，夹具须先经项目预览入口启动。浏览器核对 CLI/TUI/Web 各自的成功、审批取消、执行取消，共九例；缺任何一例都不能通过。结果保存在指定输出目录的唯一 `run-*` 子目录，提供 JSON、Markdown 和失败日志；不会写入正式模型成绩历史。

此基线不调用模型。费用、Token 和模型误报完成率保持未测量；确定性协议回归不能代替真实模型质量对照。真实 Rust/TypeScript 服务与 AST 的专项验收独立记录，不由假服务测试推定。

## 加任务

复制一个同语言的目录改名；`id` 必须等于目录名；`solution/` 只放 `editable` 里的文件；
kind=test 必须带 `mutants/`，变异只能改 `editable` 之外的实现文件。写完跑一次
`--check-tasks`，红不了或绿不了的任务不许进任务集。
