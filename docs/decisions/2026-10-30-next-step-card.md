# 下一步卡片

日期：2026-10-30。状态：设计，未实现。

轮次收尾后，除了输入框里的灰字预测，再给一张卡片：一句话加两个按钮。点击按钮不执行任何动作，只是把一段文字作为用户的下一条消息发进对话。Web、TUI、Mac 三端语义一致，呈现按端分开。卡片与灰字预测都进现有的反馈账本，供 RSI 比较两种呈现哪个更被原样发出去。

## 对照

Claude 桌面 App 在每个回合结束时画一张「下一步」卡片（不是 Claude Code CLI 的功能）。观察到的行为：

- 卡片出现在回合结束之后，不插在干活过程中间。
- 主按钮是猜的最可能的下一步，次按钮是一个真实的退路，不是主按钮的反面。
- 点击只把一段文字作为下一条消息发进对话，走正常发送路径，审批、排队、中断都不受影响。
- 不点就当没这回事，卡片不挡输入。

## 现状

三端已有灰字预测，契约对齐：

- Rust：`crates/willdeep-core/src/input_suggestion.rs`。轮次收尾后发一次小请求，最近两条用户原话加助手回复尾部，以用户口吻预测下一句。清洗掉助手口吻、超长（120 字）、疑似凭据、`NONE`。
- TUI：空输入框里灰字，Tab 填入不发送，Esc 放弃。触发在 `tui/event_loop.rs` 的轮次收尾。
- Web：`web/src/inputSuggestion.ts`，同一契约，结局上报 `/api/sessions/{id}/input-suggestion/feedback`。
- Mac：`Xedit/AgentInputSuggestion.swift`，与 Rust 是两份独立实现，提示词与清洗名单保持同步。

反馈账本 `willdeep.feedback.v1`（`docs/FEEDBACK_LEDGER.md`）已记 `suggestion_shown`、`suggestion_accepted`、`suggestion_dismissed`、`suggestion_ignored_typed`、`suggestion_superseded`，以及发送时的 `suggestion_sent_verbatim` / `suggestion_sent_edited` / `suggestion_sent_rewritten`（带 `edit_distance`）。建议行的 `prompt_bundle` 记为 `input_suggestion@…`。RSI 读的就是这本账。

## 决定

### 预测

预测请求从「一句话」改成「最多两个选项」，灰字预测保留为退路。

- 主选项：最可能的下一步。
- 次选项：一个真实的退路，没有就省略。不要主选项的反面。
- 每个选项带一句按钮文案（四个字以内）和一段点击后要发送的文字。
- 清洗沿用现有规则：助手口吻、超长、疑似凭据、`NONE` 丢弃。两个都没过就退回单句灰字；一个都没有就不显示。

提示词属于 `input_suggestion` 这个 `prompt_bundle`，改一个字版本号就变，RSI 能按版本分开比较。

### 呈现

- Web：回合收尾后在聊天区底部、输入框上方画卡片。主按钮实心，次按钮描边。点击把对应文字走现有发送路径发出去，卡片消失。输入框开始打字、Esc、新一轮开始，卡片都撤掉。灰字预测保持原样，两者同时出现时灰字让位给卡片。
- TUI：聊天区最后一行下面画同一张卡片。`1` 选主选项，`2` 选次选项，Esc 撤掉。按键只在输入框为空且没有对话框时生效。简洁模式与详细模式都画，它不属于过程显示。
- Mac：在 composer 上方加同样的卡片，主按钮用 accent，次按钮用 bordered。点击走 `AgentRuntime` 现有的提交路径。`AgentInputSuggestion.swift` 的提示词与清洗与 Rust 同步改。

### 反馈

不新建账本。`FeedbackRecord` 加一个可空的 `suggestion_kind`（`prefill` 或 `next_step`）。旧行读出来是 `null`，按 `prefill` 对待，不改现有统计的分母。

- 卡片出现记 `suggestion_shown`，`suggestion_kind = next_step`。主选项与次选项各一行，共享 `suggestion_id`，次选项多一个 `option = secondary`。
- 点击记 `suggestion_accepted`，紧接着走发送结算，一次点击自然落成 `suggestion_sent_verbatim`。这是最强的正向信号，比 Tab 填入更干净，因为中间没有编辑环节。
- 不点就打字发送记 `suggestion_ignored_typed`；Esc 或点卡片外撤掉记 `suggestion_dismissed`；新一轮顶掉记 `suggestion_superseded`。
- 灰字预测的行补上 `suggestion_kind = prefill`，其余不变。

`dwell_ms` 与 `edit_distance` 现成，不加字段。RSI 按 `suggestion_kind` 分开算「原样发出去的比例」，比较两种呈现。

## 不做

- 不在干活过程中间出卡片。过程中只有计划卡片和一行状态。
- 点击不绕过发送路径，不直接调工具或改会话。
- 不把卡片文案写进会话记录。只有点击后发出去的那段文字进记录。
- 不新建反馈文件，不改 `willdeep.feedback.v1` 的 schema 名。新字段可空，旧读端跳过不认识的键。
