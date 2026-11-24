# 历史变更记录 (0.73.0-rc4–0.78.0-rc6)

## [0.78.0-rc6] - 2026-09-20

### Changed
- **系统提示词明确要求汇报进度。** 线上一个 deepseek-v4-pro 会话 397 条 assistant 消息里 379 条只带工具调用、正文为空，全程只说了 18 句人话；原契约「avoid ceremonial preambles… do not narrate routine mechanics」被思考型模型执行得太彻底。现在改为：任务开始前一句话说打算怎么做，阶段切换（探索完、开始改、跑验证、遇到意外）时一句话更新，大约每 3 到 5 次工具调用一次；并直说进度要写在正文里，用户看不到 reasoning。
- **思维链流到界面做「思考中」行。** 新增 `ProviderEvent::ReasoningDelta`：chat-completions 流式解析器把 `reasoning_content` 增量按段冲刷（攒够 120 字或遇到换行、正文或工具调用开始、收尾时），守护进程映射为 `task.output` `type: "reasoning_delta"`。TUI 用临时行显示最后 1200 字，标签「思考中」；正文增量改标「回复中」，两种来源切换时清缓冲，不再拼成一段。思维链不进检查点、不进会话记录。

### Known issues
- Web 端暂不显示 `reasoning_delta`（`client_event` 直接丢弃，免得落到兜底分支变成标签）；Responses / Anthropic 流式尚未接 thinking 增量。

## [0.78.0-rc5] - 2026-09-19

### Changed
- **连续工具行自动收起，只留最新几条。** rc4 把每次工具调用都落进聊天区之后，几十次调用会把聊天区刷成流水账，最新进展反而要往上翻。现在同一段连续的工具行达到 6 条起，前面的收成一行汇总（`· ⋯ 已收起 12 条工具调用 · read_file×3 · run_command×9 · 2 失败 · /tools 展开`），只显示最新 4 条；模型说一句话或轮次收尾后重新计数。收起只在显示层做，记录本身一行不动：搜索跳转命中被收起的行时落在那行汇总上，选区复制以屏幕所见为准。
- 新命令 `/tools`：临时展开或收起全部工具行，`/help` 与补全菜单同步登记。

## [0.78.0-rc4] - 2026-09-19

### Changed
- **中途文字与工具调用直接进聊天区，进程内与 Runtime 轮次一致。** 此前模型在两次工具调用之间说的话只作「思考中」临时行，下一次调工具就擦掉；工具调用只进活动区，折叠态只露最近两条。线上一轮 46 次工具调用跑了九分多钟，聊天区一个字没有，用户分不清是在干活还是卡死。现在：
  - 每段定稿的中途文字作 `WillDeep:` 行落进记录；流式增量仍走临时预览行，定稿后由记录行接替。
  - 每次工具调用先落一行 `· … run_command · cargo test -p`，完成后原地改成 `✓` / `✗`。摘要沿用守护进程事件里的 `detail`，进程内轮次用同一个 `tool_detail`，不带路径与凭据；完成事件不带摘要也按名字对上发起行。
  - 收尾只追加还没显示过的部分：正常收尾与最后一段相同则不重复；撞轮次上限时只补那句「⚠ 轮次上限…」提示。非 Runtime 托管的会话镜像同样只补差量。
  - `compact_thought` 随「临时思考行」一起下线。

### Fixed
- `runtime_ui.rs` 里 `following_runtime_events_survives_a_daemon_write_mid_turn` 测试有一处从未读取的赋值，`cargo clippy --all-targets -- -D warnings` 在此处直接报错；现在旧快照改名 `stale` 并在结尾断言它确实落后于守护进程的写入。

### Known issues
- 重开会话时工具行不回放（会话记录只投影用户与助手消息），中途文字照常按记录回放；活动区侧栏不受影响。

## [0.78.0-rc3] - 2026-09-18

### Fixed
- **TUI 里 Runtime 轮次撞上轮次上限后一个字不显示，「工作中」一直挂着。** 这类没收敛的收尾，Runtime 发的是 `task.output` `type: "partial"`，再跟一条 `task.partial` / `turn.partial`。TUI 只认 `completed`：收尾文字（「⚠ 轮次上限 64 已用尽…」）被丢掉，轮次也不结束，要等快照对账强行复位，只留下一句「Runtime 已无在途轮次…已复位」。线上一整轮 64 次工具调用就这样一句回应都没出现。现在 `partial` 与 `completed` 一样落进记录，`task.partial` / `turn.partial` 结束轮次。
- **Runtime 轮次中途说的话在 TUI 里完全看不到。** `assistant_text` / `assistant_text_delta` 此前被忽略；现在与进程内轮次一致，显示为临时思考行，新一轮或调用工具时清掉，重开会话时仍以会话记录为准。流式预览长度抽成共享常量 `THOUGHT_PREVIEW_CHARS`。

## [0.78.0-rc2] - 2026-09-18

### Fixed
- **关掉 TUI 再打开，上一轮挂着的审批弹不出来，任务一直吊着。** 审批只弹回「发起这一轮的客户端」，而客户端身份是 `tui:<进程随机 id>`，发起端一退出就再也匹配不上：侧栏「需要关注」里看得见等待审批，却没有弹窗、也没法就地解掉（线上一条「重跑效果未知的 `go build`」审批因此挂了一整夜）。现在同一种界面（都是 TUI，或都是 Web）看着同一个会话也能接住这条审批；终端与浏览器之间仍只回发起端，互不代签；别的会话、工作区级视图照旧不弹。

## [0.78.0-rc1] - 2026-09-17

### Added
- **终端里也能切审批模式：`/permissions`（别名 `/permission-mode`）与 Shift+Tab。** 档位与 macOS 版逐档对齐——严格 / 智能审核 / 工作区可写 / 完全访问。
  - `/permissions` 打开面板（↑/↓、1-4、Enter）；`/permissions <档位>` 直接切；**Shift+Tab 只在严格 → 智能 → 工作区可写之间循环，永远不会落到完全访问**；选完全访问必须在确认页按 `y`。
  - 输入框标题常驻当前档位，完全访问为红色。
  - 本轮在跑时也能切，而且**对这一轮立即生效**：进程内 Agent 与 Runtime 任务共享原子档位句柄（`SharedApprovalMode`），Runtime 侧登记活跃任务句柄，新控制面操作 `session.update_approval_mode` 先持久化会话档位、再改写正在跑的任务。
  - 切档只对当前 TUI 有效；`/permissions default <档位>` 写回 `[agent] approval`（保留注释、原子写）。每次切换在 `approvals.jsonl` 记来源 `mode-change`。
- **新档位 `full-access`**：静态分类器判为破坏性的命令仍然问人，其余 Shell、MCP、网络 POST、Worktree 免审，并摘掉写入围栏；审计来源 `full-access`。配置 `approval = "full-access"` 亦可。
- 协议：`WorkspaceAccess` 增加 `strict`、`full_access`；`RuntimeSession` 增加可选 `approval_mode`；共享 fixture 已更新，Xedit 1.387.0-rc1 同步解码与对照表。Web App 工作区下拉显示五种策略。

### Changed
- **`workspace-write` 与 `smart` 不再是同一套判定。** 此前两档在核心层完全相同。现在 `workspace-write` 不请 AI 判官：静态安全命令直接跑；未分类命令只有在**内核写入围栏生效**且**看不出要出工作区**（`curl`/`ssh`/`docker`/`sudo`、`git push/pull/fetch/clone`、`cargo publish`、全局安装、heredoc / `$(…)` 等）时才放行，审计来源 `workspace-access`，否则问人。该档在 `agent.sandbox` 未配置时默认套围栏。
- 为保持升级前后行为不变：Runtime 工作区注册表里旧的 `workspace_write`（与 `smart` 同判定、多为自动登记默认值）首次启动时一次性迁移为 `smart`（`access_semantics = 2`，`schema` 不变，可回滚）；新自动登记的工作区默认 `smart`；`--full-auto` 映射为 `smart`。
- Runtime 合成本轮档位：会话档位优先于工作区策略，`read_only` 工作区是硬上限。
- 审批闸门从 `tools.rs` 拆到 `tools/approval.rs`（`tools.rs` 由 2977 行降到约 2700 行）。

### Known issues
- `full-access` 下 `create_file` / `edit_file` 仍按工作区路径解析，工作区外文件只能经 Shell 访问（macOS 版文件工具可直接写工作区外）。
- `workspace-write` 的「出工作区」判断是保守的命令形态启发式，解释器脚本（`python x.py`）内部联网看不出来；写入由围栏兜底，网络不拦。
- 没有围栏后端（未装 `bwrap` 的 Linux）或显式 `sandbox = false` 时，`workspace-write` 对未分类命令会比 `smart` 问得更多；切档时 TUI 会提示。
- Xedit 目前只读会话档位，不调用 `session.update_approval_mode`。
- `headless_runtime` 的中断恢复用例在非 UTF-8 locale 下因 Ruby 解析 JSON 报编码错误失败，与本版本无关（UTF-8 下全部通过）。

## [0.77.0-rc3] - 2026-09-17

### Fixed
- **Runtime 会话历史一旦超过容量的 75%，此后每一步模型调用都要先等几分钟的整段摘要。** 请求期压缩只改本次请求的投影、不改存储的历史，而摘要缓存按精确的切分下标命中：每轮工具调用都追加消息、下标一动就失效，于是每一步都把整个前缀从头重新摘要一遍（线上 `deepseek-v4-flash`、131K 窗口的会话，每次 114–236 秒，单次最多输出 1.3 万 token）。现在改为：
  - **增量摘要**：已有摘要覆盖 `messages[..through]` 且前缀未变时，只把「上一份摘要 + `messages[through..split]`」交给摘要器；切分点仍按工具批次边界对齐，tool_call 与其结果不会被拆开。
  - **按压缩后体积判滞回**：原始历史在一次运行内只会越来越大，因此是否需要刷新摘要看「沿用旧摘要拼出来的投影」是否再次过线；没过线就直接复用，不调摘要器，也不再发 `compression_started` / `compression_completed` 事件。
  - **前缀指纹校验**：缓存记录前缀（不含每轮会刷新的 system 消息）的哈希，前缀被改写时退回整段摘要。
  - 新增 2 例单测：连续 30 步都在水位线以上时，摘要器调用不超过 1 次整段 + 少量增量，且增量请求不再包含已摘要的原始前缀；前缀被改写会让缓存失效、system 提示词刷新不会。

### Known issues
- 摘要缓存只活在一次运行（一条用户消息）内，不写回会话：每条新用户消息到来后，超过水位线的会话仍会做一次整段摘要。把压缩结果持久化到会话需要处理 Runtime 会话的 message_generation / checkpoint 约束，留待后续。
- 未对线上会话重放验证耗时改善，只有单测覆盖。

## [0.77.0-rc2] - 2026-09-17

### Fixed
- **thinking 模型（`deepseek-v4-flash` 等）跑长任务时，中途整条请求 400：`The reasoning_content in the thinking mode must be passed back to the API`。** 0.73.0-rc5 只回传了有思维链的 assistant 消息；但 DeepSeek 在带 tools 的 thinking 模式下要求**每一条** assistant 消息都带 `reasoning_content`，而模型对 `git commit`、`git switch` 这类直给的步骤常常不吐思维链（线上会话 67 条工具调用里有 10 条为空），压缩器插入的归档引用 / 上下文摘要也没有。chat-completions 出站时，只要历史里出现过思维链，就给缺的 assistant 消息补 `reasoning_content: ""`；从没见过思维链的端点不发这个字段，严格校验的上游不受影响。Anthropic / Responses 分支没有开 thinking、不回传思维链，不涉及。

### Known issues
- 补空串是否被 DeepSeek 接受只由文档与单测推定，本版本未对线上 some.im 重放那条 400 会话。
- 首次工具调用前历史里一条思维链都没有、而上游又是 thinking 模式时，仍不会补字段。

## [0.77.0-rc1] - 2026-09-17

### Added
- **`willdeep handoff`：接住 WillDeep for macOS 交接过来的会话（配套 Xedit 1.383.0-rc1 的 `/handoff`）。** 信道是 git 远端：Mac 端把代码与会话推到 `willdeep/handoff/*` 分支，Linux 上在同一仓库的克隆里接手。两边各用各的 git 凭据。
  - `handoff list`：拉取交接分支，标出 `pending` / `imported` / `taken`（分支头没有传输文件即视为已被接走）。
  - `handoff accept [分支]`：要求没有未提交的已跟踪改动；切到分支（已有本地分支只 fast-forward）、导入会话、按 `run --session <id> --input <brief>` 续跑，daemon / 本地、审批与退出码沿用 `run`。`--no-run` 只导入并提示 `willdeep -r <id>`。
  - `handoff watch [--accept]`：轮询远端（默认 60 秒、最少 10 秒），新交接逐条接手、跑完再接下一条，并把 `--profile` / `--model` / `--config` / `--full-auto` / `--max-turns` 传给每次接手。
  - **导入不信任文件里的配置**：新建本地会话，只取对话、计划、目标与标题；`config`、`profile`、`model`、路径一律忽略；孤立工具结果清洗掉；版本不是 1、id 与目录名不符、单个传输文件超过 16 MB 都拒绝。
  - 测试用真实 git（裸远端 + 发送端 + 接收端）走完 列出 → 接手 → 已导入 → 删传输目录后变 taken，并覆盖脏工作区拒绝与 `--no-run`；样例 JSON 与 Mac 端 `WillDeepCLIHandoff.cliSessionJSON` 的输出同形。

### Known issues
- 只有 git 轮询，没有账号级推送：some.im 信道未实现，`watch` 发现新交接最多延迟一个轮询间隔。
- `watch --accept` 接手后工作区若留有未提交改动，下一条交接会因脏工作区被拒，需要人处理。
- 未在真实 Linux 机器上接过 Mac 实际推送的交接（两端都只有各自的测试覆盖）。
- `cargo clippy -D warnings` 的两处存量报错（`cloned_ref_to_slice_refs`、`unused_assignments`）仍在，与本版本无关。

## [0.76.0-rc1] - 2026-09-17

### Added
- **`monitor` 工具：命令还在跑的时候就把新输出交给模型（后台任务合同第三批 C1，canonical 在 Xedit `docs/BACKGROUND_TASK_CONTRACT.md`「C·一」）。** 对标 Claude Code 的 Monitor，替代「sleep + get_job_output」式轮询。`monitor(command, label, timeout_seconds?)` 立即返回 `mon_xxxxxx`，stdout 的新行作为 `<monitor-event>`（id / label / seq / lines + 脱敏过的代码块）推给模型，进程退出、超时、刷屏或被 `kill_job` 停掉时发 `<monitor-ended>`（reason / exit_code / duration / events / output_path）。
  - **审批与 `run_command` 同一道闸**（`gate_command`），不开新口子；只读模式直接拒。
  - **只有 stdout 是事件源**，stderr 只进日志；每行截到 2000 字符；200ms 合批、单个事件最多 50 行。
  - **防刷屏与唤醒节流长在监视器自己身上**——宿主事件不走外部唤醒额度，没有别处可放：任意 60 秒超过 30 个事件即杀进程组并以 `reason: flooded` 结束；每个监视器 30 秒只唤醒一次空闲会话，其余事件以 `enqueue` 入队、在下一个回合边界随别的事件一起交给模型，结束事件不受限。为此内核的 `pending_wake_authority` 不再把 `enqueue` 事件算作唤醒理由（这本来就是它文档里写的语义）。
  - **随宿主进程存活**，注册进进程内后台注册表：`kill_job` 可停，无头 `willdeep run` 等它结束、把事件与结束通知交给模型之后才退出；TUI 每秒刷新时按信号唤醒。日志完整落在 `~/.willdeep/monitors/<id>/stdout.log` / `stderr.log`（沿用 256MB 上限与 `.tail` / `.dropped`），运行中 `get_job_output(mon_id)` 返回状态与日志末尾。
  - 新增按行流式读取的执行原语（`execution/line_stream.rs`），与一次性捕获共用 shell 构造、沙箱、进程组与日志落盘。
  - 金样 `docs/contracts/monitor-event.v1.txt`（一条带凭据行与 `</monitor-event>` 注入行的事件、一条 exit 1 的正常结束、一条退出码未知的刷屏结束），渲染测试逐字比对；`scripts/check_background_contract.rb` 同时比对两份金样。内核对带 `notice_contract = monitor-event.v1 / monitor-ended.v1` 的工具来源事件不再整段转义框架，外部入站冒充照样转义。
- **`send_agent_message` / `stop_agent`：主 Agent 指挥自己起的后台子 Agent（C·二）。** 此前 `instruct_agent` 只接到 TUI、daemon CLI 与 HTTP，模型自己改不了主意。`send_agent_message` 把消息投进子 Agent 的指令收件箱、下一个回合边界生效，上限 4000 字符、超长直接报错不截断；`stop_agent` 停掉它，报告照常以 `status: killed` 投回。
  - **只认本会话起的子 Agent**：按父会话派工名单校验，别的会话的、编造的 id 一律报「找不到」；目标已结束时明确说「已结束，报告已投递或即将投递」。
  - 不弹审批，但每次调用写一行 `approvals.jsonl`（新来源 `not-required`，只记动作、目标与结果，不记消息正文）。
  - 子 Agent 的工具面在构建时剔除这两个工具（连同 `spawn_agent` 与恢复工具），不依赖工种白名单写对。

### Fixed
- 无头运行的后台等待循环先看「还在跑」再取结果：此前先取结果再看状态，任务在两步之间收尾时会读到「没在跑、也没结果」而提前退出，最后那条结论到不了模型。

### Known issues
- Runtime（daemon）托管的会话没有接 monitor 的唤醒：事件照常进内核、在下一个回合边界交给模型，但会话空闲时不会被拉起；daemon 路径同样没有脱离作业的发布与唤醒。
- TUI 里切换到别的历史会话后，此前启动的 monitor 事件仍记在启动时的会话名下，不会唤醒新会话。
- `~/.willdeep/monitors/` 暂无保留策略（后台作业有 7 天 / 200 个），需要手动清理。
- `cargo clippy -D warnings` 的两处存量报错（`cloned_ref_to_slice_refs`、`unused_assignments`）仍在，与本版本无关。

## [0.75.0-rc1] - 2026-09-17

### Added
- **后台任务合同 v1（与 macOS 共用，canonical 在 Xedit `docs/BACKGROUND_TASK_CONTRACT.md`）。** 对标的是 Claude Code 那种体验：命令丢后台立刻拿句柄，输出随时读尾巴，结束后在下一个边界被告知，核实完接着干原来的活。本期是 CLI 这一半：
  - **日志完整落盘。** `run_in_background` 作业的 `stdout.log` / `stderr.log` 从「每来一块就把尾部 128KB 整份原子重写」改为追加写，单条流上限 256MB；撞上限后停写，真实末尾另存 `stdout.log.tail`，丢弃字节数记在 `.dropped`。此前日志里只剩尾巴，模型拿到路径也 grep 不到前面的东西；长输出下每块都是一次 O(n) 重写。
  - **完成通知统一成 v1 格式**：`id / kind / label / status / exit_code / duration / output_path / stderr_path / omitted_bytes` 逐行，再跟一段脱敏过的尾巴——失败 40 行 / 4000 字符，成功 10 行 / 1000 字符（子 Agent 报告成功也给长的）。`status` 用 snake_case（`timed_out`、`vanished` 等），`exit_code` 取不到写 `unknown`，时长写 `1m5s`。此前进程内任务是 `status=Completed, exit=Some(0), elapsed=12345ms`，脱离作业又是一套中文标题加 4KB 字节尾巴，同一个模型两种读法。三份金样放在 `docs/contracts/background-task-notification.v1.txt`，渲染测试逐字比对。
  - **内核不再把通知框架转义成 `&lt;background-task-notification>`。** 工具输出与 Worker 报告仍是 `tool` / `model` 来源；v1 渲染器已在字段级脱敏、中和标签，事件带 `notice_contract` 元数据时内核让开这一层。外部入站（`network` 来源）冒充同名元数据照样转义。
  - **`get_job_output` 统一语义**：默认 200 行、上限 2000，脱离作业此前**忽略** `tail_lines` 固定回 16KB；返回头部带状态、退出码、时长、日志路径。
  - **系统提示词加一段后台用法**（两端逐字一致）：≥30 秒的构建/测试/发布/dev server 放后台、会自动通知所以不许 sleep 或轮询、等待时干不冲突的活、通知到了先核实再回原任务、没收到通知前不许宣称成功。
  - **保留策略**：启动时清理结束超过 7 天、或超出最近 200 个的已结束作业；还在跑的一律不动。

### Fixed
- 进程内后台 Shell 任务输出超过 64KB 时留的是**开头**，结论所在的末尾被丢掉；现在 Shell 留尾、子 Agent 报告仍留头。

### Known issues
- `cargo clippy -D warnings` 在 develop 上本就有两处存量报错（`conversation/tests.rs:141` 的 `cloned_ref_to_slice_refs`、`tui/runtime_ui.rs:661` 的 `unused_assignments`），与本版本无关，未在本次修改。

## [0.74.0-rc2] - 2026-09-17

### Fixed
- **`willdeep run`（无头）不等后台命令就退出，模型永远见不到结论。** `run_in_background` 的命令自 detached jobs 起一律落成脱离作业（`~/.willdeep/background-jobs/`），而无头等待循环只看进程内注册表：第一轮一结束，发现「没有在跑的任务」就收尾了。`docs/SUBAGENTS.md` 写的「非 TUI 模式会保持进程存活直到后台任务完成」与代码不符。现在循环同时看本会话名下的脱离作业，结束后作为运行时事件再交给模型一轮。
- **TUI 里后台命令跑完了，空闲会话却不被唤醒。** 每秒轮询把结论放进了事件队列，但没像进程内任务那样调 `wake_for_kernel_events`，结论要一直等到用户下次开口。现在发布即唤醒（仍受每 5 分钟 6 次的外部唤醒额度约束），并响铃提示。
- **`kill_job` 停不掉 `run_in_background` 起的命令。** 它只查进程内注册表，对脱离作业一律报「找不到」。现在回落到作业记录：核对 PID 启动时刻后向 supervisor 发 SIGTERM，supervisor 连同命令进程组一起收掉并落下退出码 137；已结束的作业明确回「已结束，用 get_job_output 读」。
- 顺带修掉一个会被上面唤醒放大的问题：TUI 此前把**整个作业目录**里所有已结束作业都发布给当前会话——别的会话、几周前的作业也算。现在作业记录带 `owner`（起它的会话），通知只投给主人；投递权用作业目录里的 `delivered` 标记（`create_new`）领取，TUI、无头运行、重启之后同一次结束都只讲一遍。升级前留下的无主旧记录不再补投。

## [0.74.0-rc1] - 2026-09-16

### Added
- **插件页面问模型时可以附图片（桥 2.6.0，能力 `ai.images`）。** 与 macOS 宿主 1.377.0-rc1 对齐：`ai.complete` 的 user 消息新增 `imagePaths`，填本插件媒体目录里的本地路径，宿主解码、长边限到 1568 像素、转 JPEG 后作为图片附件送进模型。起因是短剧工坊要对定妆图和首尾帧做多模态内容审核，此前消息只收字符串，选了识图模型也递不进图。
  - 路径钳制：必须是绝对路径、普通文件、非符号链接，规范化后父目录恰好是本插件的媒体目录（`plugin-media/<plugin>`），否则报 `mediaOutsidePluginData`。刻意比参照图那条路（插件媒体总目录 + 工作区）更严：这里是把文件内容发给第三方模型。
  - 上限与拒绝理由：一次最多 12 张图（`tooManyImages`）；附件挂在 system / assistant 上整条拒（`mediaOnNonUserMessage`）；解不出来的文件报 `unreadableMedia`。全部在调用模型之前判定。
  - **不声明 `ai.videos`**：本宿主没有视频解码器抽帧，带 `videoPaths` 的请求报 `videosUnsupported`，不静默丢掉——丢掉的话模型会在没看到画面的情况下给出审核结论。macOS 宿主声明了这一项，由它抽帧。

## [0.73.0-rc8] - 2026-09-17

### Fixed
- **TUI 对话挤成一坨，读着累**：用户输入结束后、AI 回复结束后各留一行空白，一问一答之间有了呼吸感（`tui/rendering.rs`）。滚动高度按同一份渲染结果计算，不会错位。

## [0.73.0-rc7] - 2026-09-15

### Fixed
- **治本：把视图状态从执行指纹里拿出来。** `attention_read`、`runtime_event_cursor`、`runtime_managed` 三项此前算在执行快照里（`session/execution_state.rs`），于是「客户端记一下自己读到第几条事件」与「守护进程写入这一轮的输出」被判成执行冲突——而一条会话同时被这两者持有，正是运行时托管的**常态**。结果是每次模型输出到达必有一边被打死：rc3 之前是 TUI 在「结果刚要显示」时退出；rc4 把 TUI 那侧改稳之后，换成守护进程的回合直接失败（`failure_domain=internal`，错误正文相同）。现在三项移出指纹，并在 `metadata::merge` 里按各自语义合并——游标取较大者、已读标记取并集、托管标志一旦置位就保持。真正的执行冲突（两个写者拿着各自的历史续写）仍然照拦。
- rc2 起连续三个版本都在治标：rc2 修启动路径、rc3 修本地回合完成、rc4 修跟随运行时事件，三处都只是把同一个错误挪了个地方。根因是数据模型把三类不同的字段混进了一份指纹。

## [0.73.0-rc5] - 2026-09-15

### Fixed
- **思考型模型一旦用上工具就必挂**：DeepSeek（`deepseek-v4-flash` 等）在 thinking 模式下要求把上一轮的 `reasoning_content` 原样回传，而 chat-completions 分支从头到尾没有这个字段——历史里只要出现过一次工具调用，下一轮请求就是 400 `The reasoning_content in the thinking mode must be passed back to the API`，会话就此卡死。现在流式与非流式解码都收 `reasoning_content`（兼容把它平铺成 `reasoning` 的网关），随 assistant 消息回传；思维链只用于回放，不混进正文、不当作模型给用户的回答。桌面端会话导入同样带上。
- **`function.arguments` 不是合法 JSON 对象时，整条历史被上游拒掉**：实测同一条截断的 `{"path": `，一批上游照收，另一批直接 400（`Assistant tool call function.arguments must be valid JSON.` / `Expecting value` / `Can only get item pairs from a mapping`），拒的是整条历史而不是那一个工具调用。流式解码器本来就拦这一手，但非流式解码与旧版桌面会话导入两条路没有。现在出站前统一兜底：空串补 `{}`，非对象的原文装进 `_raw_arguments` 保留证据；`sanitize_tool_history` 回放持久化历史时一并修。

## [0.73.0-rc4] - 2026-09-15

### Fixed
- **守护进程执行回合时，TUI 在「AI 输出刚要显示」的那一刻退出**，报 `concurrent session update conflicts on execution`。跟随运行时事件的那段是 `load → 改游标 → save`：基线在 `load` 那一刻定格，而守护进程每收到一块输出就写一次会话文件，`save` 时磁盘早已又变——游标又算在执行指纹里，于是**每次输出到达都可能把 TUI 打死**，那一轮的结果连同 token 一起丢。改为在会话锁内从最新快照起改（`SessionStore::update`），守护进程刚写进去的消息全部保留，这里只叠加自己的游标与已读标记。「空会话不为游标落盘」的既有规则保持不变。
- 这是 rc2 / rc3 两次没修对的同一个症状：rc2 修的是启动路径、rc3 修的是**本地**回合完成路径，而用户那条会话由运行时托管，走的是第三条路。定位依据是报错时刻与会话文件 mtime 的交叉比对。

