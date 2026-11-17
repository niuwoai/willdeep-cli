# TUI 队列时间线本地验收

日期：2026-11-17。版本：`0.151.0-rc3`。本次为既有 TUI 的队列显示修复，未安装、提交、推送或发布。

## 行为与边界

成功入队的反馈保留在消息投递状态和临时状态区，不再永久追加 `System` 排队回执。首次有序 `queued_user_message` 事件到达时，先结清上一轮的流式/暂存输出，再将对应用户记录移至末尾，使对话按「上一轮完整输出 → 实际开始的用户消息 → 对应回复」排列。

状态快照与正式落位分离；不按提交时间、相同正文或队列缺席推测开始顺序。重复事件不会再次移动记录。移动保留原始时间、附件、模型与稳定来源身份，重映射队列回执、展开、搜索和归档索引，并保留阅读锚点及选区原文。丢回执后的身份合并保留已确定的正式位置；主动清除的记录继续作为身份墓碑，不会因回放复活。

## 检查结果

下表及固定二进制记录是首次队列修复的阶段验收。独立复核随后发现「丢回执身份合并时跨记录选区高亮与复制原文可能不一致」，现已用真实身份对账及鼠标入口补充回归；历次失败与修补过程见“身份合并选区补充回归”。先前 640 项原生测试和 10 项 PTY 检查不作为新增选区场景的通过证据。

| 检查 | 结果 | 范围 |
|---|---|---|
| TUI 原生测试 | 640 通过，0 失败，2 忽略 | 同文不同 Turn、快照先到、重复事件、身份合并、消息移位、附件时间、full/compact 阅读与选区及既有 TUI 回归 |
| 工作区内部依赖版本测试 | 1 通过，0 失败 | Runtime 协议及客户端依赖 version 钉 |
| CLI 严格 Clippy | 通过 | `cargo clippy -p willdeep --bin willdeep -- -D warnings` |
| CLI 开发构建 | 通过 | `cargo build -p willdeep --bin willdeep` |
| 队列时间线真实 PTY | 10 通过，0 失败 | full/compact 各 5 项；真实二进制与持久 Runtime，回环合成 SSE Provider |
| 版本元数据、源码长度、定向 rustfmt、差异空白 | 通过 | `0.151.0-rc3`，手写文件不超过 3000 行 |

原始输出：[TUI 原生测试](queue-native-tests.log)、[内部依赖版本](queue-workspace-versions.log)、[严格 Clippy](queue-clippy.log)、[CLI 构建](queue-build.log)。

两项忽略测试是仓库已有的手工流式性能测量，本轮未执行。当前记录不代表全工作区、真实模型、其他操作系统或 `/local` 的端到端验收。

### 真实终端专项

验收固定副本：`target/debug/willdeep-queue-timeline-rc3`。

SHA-256：`e7910b9275d4cb85b6389cc6e5a6ea896a65f7aa6139dd7287e80b4251e479a3`。脚本核对验收期间二进制摘要未变化；副本与共享构建输出分开，避免并行构建改变受测二进制。

两种布局分别执行：保持第一轮流式回复未结束时，连续提交两条待发消息，并在编辑器保留一条未发送草稿；补发第一轮尾段，逐轮开始后两条消息。

验证：

1. 第一轮结束前，后两条不会提前触发 Provider 请求。
2. 第一条出队用户消息位于上一轮尾段之后、对应回复之前，聊天区只出现一次。
3. 全部用户消息及回复遵循实际轮次顺序，两条同类待发消息均不重复。
4. 过期排队回执消失，未发送草稿保留，草稿不会触发额外请求。
5. Runtime 保存的会话历史与显示顺序一致。

完整布局的活动区也会预览待发文本，因此去重/顺序断言限定在聊天区；状态清理与草稿断言仍检查全屏。最终采图等待周期性队列快照追上真实开始事件，避免展示短暂的旧队列计数。

产物：

- [JSON 报告](queue-timeline-verified/queue-timeline.json)
- [Markdown 报告](queue-timeline-verified/queue-timeline.md)
- [完整布局终端帧](queue-timeline-verified/full/03-all-dequeued.svg)
- [简洁布局终端帧](queue-timeline-verified/compact/03-all-dequeued.svg)
- 对应目录保留各阶段的原始 ANSI 与解码屏幕 JSON。

夹具使用私有 HOME、WILLDEEP_HOME 和工作区；只连接本机合成 Provider，使用测试配置，不继承生产凭据。OSC 52 仅由私有 PTY 捕获，不写宿主剪贴板。仅停止自己启动的临时 Runtime。

### 重现命令

```bash
cargo test -p willdeep --bin willdeep tui:: -- --test-threads=4
```

```bash
cargo test -p willdeep --test workspace_versions
```

```bash
cargo clippy -p willdeep --bin willdeep -- -D warnings
```

```bash
ruby scripts/verify_tui_queue_timeline.rb target/debug/willdeep-queue-timeline-rc3 docs/verification/0.151.0-rc3/queue-timeline-verified
```

```bash
ruby scripts/check_release_version.rb 0.151.0-rc3
```

```bash
LC_ALL=en_US.UTF-8 LANG=en_US.UTF-8 ruby scripts/check_source_size.rb
```

## 身份合并选区补充回归

补充回归均调用真实的 `reconcile_runtime_deliveries`，覆盖 full/compact、正反两个选择方向，并在合并前、合并后和再次绘制后分别校验复制原文与高亮：

- 原占位消息尾部至后续消息头部，包含纯正文与实际助手角色前缀。
- 后续消息至已正式定位副本的头部，原占位不参与选择。
- 两副本各选不同字节、中间多条消息、中文文本改宽、先移动再合并，以及合并后新拖选和清除。
- 只选正式副本正文至下一条消息；合并从未选中的原占位补入文本附件后，旧选区不应包含新增附件。
- 使用真实 `MouseDown`/`Drag`：合并后两端重合或不重合，再从同一屏幕位置开始选择，必须丢弃旧的跨消息缓存。仅调用测试辅助函数设置相同端点不足以证明鼠标路径有问题，因此另测实际入口。

修补采用每个原有来源、部件和有限字节范围的快照；身份合并仅迁移范围的来源 ID。同一来源可保留多个不连续片段，高亮取这些片段的并集，复制继续使用原文缓存；新拖选或清除会重置快照。鼠标按下通过唯一的新手势入口重置旧快照，普通重绘及记录移动继续保留已有选区。

- [原缺陷复现](willdeep-selection-merge-before-regression-9a2622ac.log)：应只高亮 `beha`，实际为 `etagammaalph`，错误带入未选的 `gamma`；复制缓存仍为 `ha\nbe`。
- [候选补丁反例](willdeep-selection-merge-survivor-with-freeze-9a2622ac.log)：直接冻结整个来源顺序并沿用首次去重，会将正式副本的选区前移。应高亮 `betagammaal`，实际为 `blpha`。
- [撤回后的完整定向回归](willdeep-selection-merge-final-rollback-suite-9a2622ac.log)：9 项通过、1 项失败。两个测试均保留，候选实现已撤回。
- 范围迁移的中间实现：[记录顺序 11 项通过](queue-selection-intermediate-order.log)、[完整 TUI 645 项通过、2 项忽略](queue-selection-intermediate-tui.log)。这两份结果早于附件边界修补，不是最终实现的完整验收。
- [新增附件反例](queue-selection-attachment-before.log)：只选择正式副本正文时，应高亮 `alphag`，实际还带入附件标签及内容。修补后按每个原有部件的有限字节范围冻结。
- [调用点漏传参数](queue-selection-compile-before.log)：调整范围生成参数时，普通高亮调用点漏传部件表，编译失败，未进入测试；随后补齐两处调用。
- [角色前缀回归](queue-selection-prefix-before.log)：仅用原文部件表会漏掉有来源映射、但未单列原文的助手角色前缀；定向回归为 11 项通过、1 项失败。现以完整原文长度为基础，补充原帧映射中的部件字节边界，保留折叠正文和角色前缀。

- [真实鼠标反例](queue-selection-mouse-before.log)：两个副本的选区端点在合并后重合；再次 `MouseDown` 同一点，应只选中 `l`，实际仍保留 `lpha\nbeta\ngamma\nal`。新手势入口现先清除旧选区缓存，再捕获当前单点，后续真实拖动可选择 `lph`。
- [Core 中间态阻断](queue-selection-core-compile-blocker.log)：额外重编 viewport 测试时，另一会话尚未落齐的 Core 接口导致编译失败，未进入测试。本轮未修改该模块；保留准确失败，随后在已编译的固定测试副本上验证视口行为。

最终补充检查：

| 检查 | 结果 | 证据与边界 |
|---|---|---|
| 最终记录顺序与真实鼠标回归 | 13 通过，0 失败 | [完整输出](queue-selection-final-order-with-mouse.log)，包含全部原断言和新增的附件、双副本、同落点新手势场景 |
| 最终 viewport 回归 | 16 通过，0 失败 | [完整输出](queue-selection-final-viewport-with-mouse.log)，运行刚通过上述 13 项的同一测试二进制固定副本 |
| 独立只读核对 | 已处理具体发现；新手势入口核对 PASS | 附件部件上界、同落点鼠标缓存均有先红后绿测试；最终静态核对确认清理仅发生于新的左键 Down，未另跑测试 |
| 两文件 rustfmt、差异空白 | 通过 | 仅格式化独占选区文件；无源码长度超限 |

最终测试固定副本为 `target/debug/willdeep-queue-selection-rc3-bx3rze10b-tests`，SHA-256：`00f9e730836a8246080baa7181f5dca0f13960e325a8bee2117fa33c1e1ac050`，与 13 项通过后的原测试产物一致。它是 Rust 单元测试产物，不是应用运行二进制。

最终独占源码 SHA-256：

- `crates/willdeep-cli/src/tui/viewport.rs`：`e84a9b275048637f071fcee24da44e4bb9acfa4abcb1334513c0fbfe486c59d7`
- `crates/willdeep-cli/src/tui/transcript_order_tests.rs`：`08abe81e5d6be42f5ca57a5c33b6b84c6298904b1a5f9db468a1820fdd0be7b9`

上述历次失败均保留为修补前证据。中间实现的 645 项完整 TUI 通过早于最后边界修补；最终完整 workspace 与三套固定应用二进制 PTY 由整合会话统一执行，不将这里的 13 + 16 项扩大为全项目通过。未重建原有应用固定二进制，也未重跑或改标其 10 项 PTY 报告；未安装、提交、推送或发布。

## 保留的失败与修正

- 旧 `0.151.0-rc2` 二进制真实复现用户问题：出队用户行仍排在上一轮输出之前，成功入队的系统回执仍残留。首个 compact 顺序检查失败，未继续完成 full；见 [修复前报告](queue-timeline-before/queue-timeline.json)。这份失败没有改标为通过。
- 首轮原生回归为 637 通过、2 失败、2 忽略。两处旧断言仍要求永久 `System` 插话回执；现改为断言临时状态提示存在、对话中无该系统行，随后完整 TUI 回归通过。
- 首次 rc3 构建遇到并行 Runtime 修改尚未落齐的 `completion_foreground` 缺失。该模块由负责整合的会话补齐后重新构建通过，本轮没有改写其源码。
- 首次 rc3 PTY 的 compact 五项通过，full 去重断言把活动区队列预览也算入聊天消息而失败；见 [首次修复后报告](queue-timeline-after/queue-timeline.json)。已限定聊天区计数，使用相同固定二进制重跑两种布局通过。
- 源码长度脚本在默认 US-ASCII locale 下报字符编码错误；显式使用 UTF-8 后通过，未改动无关的长度检查脚本。

更广泛的 rc3 交互与输入区复制 PTY 由已有整合会话在最终组合副本上执行，其结果不计入这里的 10 项队列专项检查。
