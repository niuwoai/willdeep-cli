# 压缩模型、识图模型与不设限的轮次

日期：2026-11-01。状态：设计，未实现。

三件各自独立的事：压缩和识图各用自己的模型，主会话的轮次可以不设上限。压缩要能指定 provider、模型和输出 token 上限；主模型没有识图能力时，先把图片交给一个视觉模型看一遍，把识别结果附在提示词后面再交给主模型。参照 `~/Sites/Xedit` 的做法。

## 现状

- 压缩走主会话当前的模型。`[agent]` 里只有预算类的闸：`token_budget`、`goal_token_budget`、`goal_wall_clock_minutes`、`goal_max_continuations`。压缩输出多少 token 没有单独的设置。
- Provider 配置在 `[providers.<name>]`，已有 `model`、`max_output_tokens`、`context_window`，另外已经有一个 `vision_model` 字段（`crates/willdeep-cli/src/config.rs` 的 `ProviderProfile`），但主模型不能识图时并没有用它先看一遍图片。
- Xedit 的做法：主模型没有识图能力时，图片先交给视觉模型识别，识别出的文字附在提示词后面，再交给主模型。WillDeep 与 Xedit 的运行时不共享，这里只对齐这个行为，不共用代码。

## 决定

### 压缩

在 `[agent]` 下加三项，都可空，空了就用现在的行为：

- `compression_provider`：用哪个 provider 配置压缩。空着用主会话的 provider。
- `compression_model`：压缩用的模型。空着用那个 provider 的默认模型。
- `compression_max_output_tokens`：压缩输出的 token 上限。空着不额外限制，仍受那个模型自己的 `max_output_tokens` 约束。

压缩失败时退回主模型再压一次，并在聊天区的压缩行里说明这次换了模型。不因为压缩模型配错就让整轮停住。

### 识图

沿用已有的 `vision_model`，补上触发条件：

- 主模型声明支持图像输入时，图片原样交给它，不绕道。
- 主模型不支持时，图片交给同一个 provider 的 `vision_model` 识别，识别出的文字以一段引用附在用户消息后面，再交给主模型。图片本身不再传给主模型。
- `vision_model` 没配，或者识别失败，就保持现在的行为：图片照旧传，主模型看不懂由它自己说。不因为识图失败而丢掉用户的图片。

识别结果进会话记录，重开一个会话时不再重新识别。

## 轮次不设限

主会话的轮次上限是 `[agent] max_turns`，缺省 200，校验只接受 1 到 1000（`crates/willdeep-cli/src/config.rs` 的 `DEFAULT_MAX_TURNS` 与 `MAX_TURNS_CEILING`）。agent 循环是 `for turn in 1..=max_turns`（`crates/willdeep-core/src/agent.rs`），触顶不判失败，交出已有的部分结果。没有 256 这一档。

goal 模式另有一套闸，和轮次上限叠加，不是替代：`goal_max_continuations` 默认 64，`goal_wall_clock_minutes` 默认 240，`goal_token_budget` 默认不限。三个里任何一个先到就软停。

### 决定

- `max_turns` 允许空，空着表示不限。不限时循环不再有上界，停下来只靠模型自己收口、用户中断，或者 goal 的预算。
- goal 模式单独一项 `goal_max_turns`，只在 goal 里生效，允许比普通会话更大，也允许空。空着时 goal 会话不受轮次上限，仍受 wall-clock 和 token 预算约束。
- 不改默认值。不写配置的人拿到的还是 200 轮、goal 64 次续推。放开是显式的。
- 触顶的行为不变：到了上限交出部分结果，不判失败。

## 不做

- 不在这一版里做。侧栏改树和工具行展开是 `0.135.0-rc1`，压缩、识图和轮次不设限单独排期。
- 不让压缩模型和识图模型互相顶替。一个负责缩短上下文，一个负责看图，配错一个不该影响另一个。
- 不把识别出的文字当作模型自己的回复。它是附件，标出来源。
