# P0/P1 与 TUI 整合验收

日期：2026-11-17。版本：`0.151.0-rc2`。项目阶段：既有 Rust Runtime 项目迭代。

当前状态：四条 Runtime 状态路径的原始反例修复后，定向执行 237 次、去重 215 项通过；独立复核 73 项通过，但另指出 Turn/Session 写盘失败重放和已认领未绑定轮次预留两个边界，正在补反例验证。固定只读二进制组合验收还暴露出浏览器首次导航后的页面代次问题及 TUI 复制确认仅保留一帧的问题。本目录保留 rc2 的实际经过；同目录队列改动已升 rc3，最终组合验收将按稳定后的版本单独记录。尚未安装或发布，未创建发布 tag。

本轮范围来自 [Pi/OMP 差距清单](../../research/2026-11-17-willdeep-cli-gap-analysis.md) 的三项 P0、六项 P1，并按追加授权一并保留另一会话的 TUI 改动。配置与平台限制见 [Runtime 工具指南](../../P1_RUNTIME_TOOLS.md)。rc1 和 0.149 系列报告保留各自当时的语境，不作为本版本重复执行的证据。

## 逐项对照

| 原始项目 | 交付与判据 | 本轮状态 |
|---|---|---|
| P0 编辑版本与多文件协议 | 实际读取 SHA-256、陈旧编辑拒绝、一次性预览、审批与失败恢复、真实遗留写集 | 待最终组合回归 |
| P0 六类核心场景基线 | 陈旧编辑、跨文件引用、网页交互、压缩约束、多任务恢复、错误完成；精确测试名和非零执行 | runner 自检通过；完整执行待验收 |
| P0 LSP 查询 | Rust/TypeScript 定义、引用、符号、文档刷新和诊断；Rust 实际项目 readiness | 整合后真实 Rust 3/3、TypeScript 4/4 通过 |
| P1 语义重命名与 AST | 两阶段发现/同步、版本或原生 TS 快照绑定、多文件应用；Rust/TypeScript/TSX AST | 整合后真实 Rust/TS 重命名及原生 AST 1/1 通过 |
| P1 浏览器 | CLI/TUI/Web 各验证成功、审批取消、执行取消；业务断言、PNG、终态、工具账本和进程清理 | attempt4 九例全通过；后续收口源码使用固定二进制再验 |
| P1 BYOK 搜索 | SearXNG 与 SomeIm、独立认证、固定地址、审批、响应预算及取消 | 整合前本地 HTTP 12 项通过；组合回归待执行 |
| P1 项目模型策略 | 真实发送前 Provider/model/端点门禁、持久项目归属、运行期只收窄、切换/恢复/辅助调用 | 生产策略保持失败关闭，旧测试替身已修复；组合回归待执行 |
| P1 证据与审计 | 真正执行引用、Goal/attempt/会话/工作区/状态/快照绑定、失效重验、审批结果 Hook | 已合并上游 invocation 与 detached 验证；组合回归待执行 |
| P1 工具默认体验 | 五类离线状态、统一权限原因、MCP 脱敏观察、doctor 配置与依赖并查、审批持久化回执 | 相关脚本自检通过；组合回归待执行 |
| 并行 TUI 改动 | 真实阶段、投递身份、阅读来源锚点、选区与复制；Ctrl+X 经同一 Runtime 停止 | 保留全部实现与历史素材；组合原生及真实 PTY 待执行 |

## 实际检查

- 合并后 Web：52 个测试文件、430 项测试通过；构建成功；lint 为 0 error、1 条既有 `ModelPicker` Hook 依赖 warning。构建保留 chunk 大小提示。
- CI 的 Ruby 自检链：模型评测 32 runs / 748 assertions；P0 夹具 19 / 116；固定 runner 13 / 127；浏览器辅助检查 11 / 41；均为 0 failure、0 error、0 skip。任务清单检查通过。
- `--check-manifest` 只验证登记，输出 `executed=false`、`baseline_complete=false`，不能替代完整六类执行。
- 统一版本检查通过；篇幅检查覆盖 1,664 个手写文件，无文件超过 3,000 行。最终源码稳定后再次核对。

- 上游整合后全工作区 Rust 测试命令退出 0：CLI 单元 1,756 通过、5 忽略；core 单元 1,306 通过、7 忽略，其他集成及文档目标均通过。最终收口修复新增的回归另行执行。
- 整合后真实 Rust 3/3、TypeScript 4/4、原生 AST 1/1 通过；Rust 使用已填充的独立分析缓存并设 `CARGO_NET_OFFLINE=true`。AST 首次误选 npm 包装入口被依赖门禁拒绝，改用同一固定包中的原生可执行文件后通过，未放宽工具选择规则。
- 较早的 rc2 二进制真实 PTY 两套检查共 32/32 通过：交互 18 项、输入选区与 SSH 复制 14 项。随后固定副本 `b416dafd…fb6c06d` 的交互 18/18 通过，输入选区在 8 项通过后于 SSH 复制确认处超时；实际 OSC 52 原文与短暂确认帧均存在，产品提示仅保留一帧的问题已由反例确认。该轮不计为最终 32/32 通过。
- 浏览器启动 Node 协议测试 11/11 通过，`node --check` 与 `cargo fmt --all -- --check` 通过。真实 Node/Chrome 公开 Registry 三项补充验收也已通过：表单/引用/截图/路由与来源拒绝、页面 DOM 防伪和切只读后关闭、审批取消销毁所属会话。

- Goal 真实 ToolRegistry 组合回归 2/2 通过：detached 启动立即撤销旧 Goal 引用，普通账本与 journal 恢复不能签发进程内权限；进程内后台成功结算后返回新引用并完成 Goal。私有 supervisor 在异常路径也由测试清理守卫停止并等待退出。
- 原审批取消修复的 TaskManager、AgentStore、SessionStore、deferred gates 定向测试合计 64 项通过；独立复核另复跑 28 项通过。随后四条迟到事件与失败重放路径修复后执行 237 次、去重 215 项通过，独立复核 73 项通过；复核留下的 Turn/Session 后段写盘失败与已认领 Queued 预留边界转入 rc3 验证，不能用前述通过数替代后续验收。
- 本机全工作区严格 Clippy、Windows GNU core 全目标严格 Clippy 均退出 0；此结果早于后续状态收口修复，最终检查另行记录。

最终固定二进制的三端浏览器、完整六类及后续状态修复结果在完成后补入本节及配套 JSON。

## 失败与修复记录

1. 真实 Rust 首轮在项目尚未就绪时返回空定义、干净诊断或无引用。初始化现要求 `rust-analyzer` 的健康且静止通知，请求前后核对 readiness revision；允许首次 VFS 之后的 Cargo 元数据重载，继续关闭构建脚本、过程宏和自动检查。无通知、加载中或请求期间重载均失败关闭。
2. 独立 Rust 分析缓存缺少 metadata 依赖时，服务报告 warning，测试按约定失败；没有放宽 readiness。补齐临时分析缓存后，真实 Rust 3/3、TypeScript 4/4、AST 1/1 通过。Rust 重命名后由独立 `rustc --test` 运行行为验证，TypeScript 由 `tsc --noEmit` 检查。
3. 全工作区首轮：CLI 1,747 通过、1 失败、5 忽略；core 1,263 通过、0 失败、7 忽略。唯一 CLI 失败为旧委派测试替身无法携带 `ModelPolicyContext`。修复替身后，第二次仅路径断言在 macOS `/var` 与 `/private/var` 的规范路径差异上失败；已规范化期望路径。生产策略的 `provider_cannot_bind_workspace` 门禁保持有效，最终复验单独记录。
4. 浏览器 attempt1 九例失败：夹具位于主仓库的忽略目录，Git 向上发现父仓库，验收快照错误引用主仓库路径。夹具现建立独立 Git 工作区，历史失败报告保留在 `target/browser-entrypoints-rc2-attempt1/`。
5. 浏览器 attempt2 九例失败：CLI 的 `WILLDEEP_LANGUAGE` 覆盖使 `run` 走本地 Harness，未产生共享 Runtime Turn；语言现放进私有 `[agent]` 配置。首项浏览器还遭遇桥内固定约 5 秒 CDP 启动等待，缺少 Chrome 启动诊断。后续 TUI/Web 六例被共享夹具已有错误拦截，不能视为六个独立 Chrome 失败。失败报告保留在 `target/browser-entrypoints-rc2-attempt2/`。
6. 浏览器 PNG 位于宿主生成的 `artifacts/browser-UUID/` 子目录；验收 inventory 改为递归扫描，并同时检查隐藏的未提交产物，没有削弱额外文件、哈希、终态或进程清理断言。
7. 浏览器 attempt3 原报告标记 7/9 通过。CLI 与 TUI 的审批取消实际已清理浏览器和工具，但 Turn 在记录终态后又变成 `running`，两项等待终态超时；Web 曾观察到瞬时取消而被标为通过，后续采集的 Turn 也已回退，不能据此认定其取消状态可靠。产品状态竞态按根因修复，验收增加工具和资源清理后的终态复查，不放宽取消断言；原报告保留在 `target/browser-entrypoints-rc2-attempt3/`。
8. 收口中的一次并发执行由 Cargo 重新生成 `target/debug/willdeep`：TUI 输入选区 14 项断言均通过，但结束时二进制摘要已变，报告按约定失败；六类基线源码保持稳定且五类通过，浏览器在正式执行前被旧夹具的摘要门禁拒绝。这两份结果不计为最终通过；最终验收使用固定只读二进制副本与全新夹具，不关闭摘要校验。
9. develop 合并的 16 处文件冲突已逐段解决：保留 rc11 的调用错误分类、后台终态收集和恢复限制，以及 rc12 的 Runtime 租约顺序与插件按当前字节校验；同时保留本轮全部 P0/P1 和 TUI 来源映射、时间模型归属与 Ctrl+X。各历史版本条目均保留。

原始终端帧 `.txt` 的尾空格是布局证据，未为通过空白检查而改写；源码与其他文档仍执行 `git diff --check`。

## 保证边界

- 文件 SHA-256 是内容版本；逐文件原子安装不构成跨文件事务，也不构成对任意外部写者的文件系统 CAS。
- TypeScript 固定服务的原生 `changes` 只按请求前同步文本绑定，不伪造服务端版本；Rust 继续要求版本化 `documentChanges`。
- 浏览器页面来源拦截不构成 Chrome 全进程网络隔离，当前要求有效 OS sandbox 为 Off；非 Unix 截图仍拒绝。
- 项目模型策略约束接入的 Provider HTTP 出口，不约束任意插件或外部进程自行联网。
- detached 验证终态可进入验证账本，落盘记录本身不创建 Goal 的进程内引用权限；后台启动不是通过证据，重启缺真实权限仍需重验。
- 固定回归与本地合成 Provider 不测真实模型质量、费用、Token 或竞品胜负；这些字段保留未测，未调用付费模型或外部付费搜索。
- Windows GNU 交叉 lint 与 Windows 原生执行分别计量；本轮不恢复 Windows CI 或发行，不实现 DAP、Computer Use/AX、语音、ACP、后台预压缩或 `/btw`。

## Git 与交付

实现快照：`662e69c8`。develop 整合提交：`2744e575`。分支：`feat/p0-p1-runtime-tools`。

最终验收与文档收口提交、推送和 PR 状态在完成后记录。发布 tag 只能指向已合入默认分支的提交；本轮不通过自动合入默认分支来满足 tag 条件，也不安装或发布。
