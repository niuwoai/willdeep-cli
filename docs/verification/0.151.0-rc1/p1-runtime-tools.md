# P1 Runtime 工具与真实执行证据验收

- 日期：2026-11-17；开发版本：0.151.0-rc1。
- 状态：本地分层验收通过；真实 AST/Rust 语言服务、Windows 原生与外部服务等未覆盖范围保留在文末。
- 基线：`ba894a5906982921411e33486fa699f6f356e43e`，分支 `fix/tui-queue-during-compression`，包含未提交改动。并行 TUI 工作予以保留，不归因于本任务。
- 机器可读结果：[JSON](p1-runtime-tools.json)。历史结果：[rc3](../0.150.0-rc3/clippy-approval-hooks.md)。
- 未提交、推送、打 tag、创建 PR、安装或发布；未自动安装依赖，未运行付费模型或外部搜索。

## 最终结果

| 检查层 | 最终结果 |
| --- | --- |
| core 全套 | 1248 项单元 + 52 项普通集成通过，0 失败；单元 7 项与集成 7 项显式忽略，原因单列。 |
| CLI 相关回归 | 按完整测试名去重 224 项通过；一个手工诊断入口忽略；未宣称整个工作区测试全套运行。 |
| 实际 TypeScript 服务 | 3 项通过，含跨文件字符串键与 UTF-16 重命名预览/应用。 |
| 实际浏览器 | Rust 宿主 3 项通过；Ruby Node/CDP 桥 11 项通过，两层分别记录。 |
| 严格工程检查 | 全工作区 Clippy、core Windows GNU 交叉 Clippy、格式、版本、源码篇幅、脚本语法与变更空白均通过。 |
| 文档与报告 | 十份文档 349 个相对链接无缺失；产品概览 17170 字符，未超过 20000 字符；JSON 计数与已完成任务输出核对一致。 |

core 单元的七个忽略项包含内部子进程入口、付费 live-fire、显式语料预检及账本基准；内部入口由父测试另行调用。普通集成忽略项为一个真实 AST 用例、三个浏览器和三个 TypeScript 用例，后六项已在显式真实批次执行通过。没有自动运行付费项或安装缺少的依赖。

## 交付范围

| 能力 | 实现及边界 |
| --- | --- |
| 语义重命名 | 发现目标后同步全部 overlay，最终请求基于已保留文本；版本化响应严格匹配 URI/version。固定 TypeScript 原生 `changes` 单独按请求前快照绑定，不伪造服务端版本。最终预览/应用复用既有 `file_changes`、一次性审批和 SHA-256 版本。 |
| AST | 固定 `ast-grep`、私有空配置、stdin 文本快照，限定 Rust/TypeScript/TSX；动态权限复核、有界协议与结果后快照复核，不开放改写。 |
| 浏览器 | 内置 Node/CDP 桥、独立 Chrome profile、固定动作、动作审批、代次/ref、隔离 DOM 执行环境、真实 CDP 状态、业务断言及截图元数据。Unix 使用目录句柄落盘，取消清理未提交产物与所属进程树。 |
| 独立搜索 | SearXNG 与 SomeIm 兼容；独立配置与环境密钥引用，审批先于 DNS，固定核验地址，禁代理/跳转/重试，1 MiB 响应硬限和统一期限。 |
| 项目模型策略 | 最终模型、Provider 与端点共同匹配，可信允许集在同进程只可收窄；持久项目归属传到恢复、worktree、Worker、辅助模型、切换、回退及 HTTP 出口。策略启用时跳过模型未知的自动发现，图片下载非 2xx 不落盘。 |
| Goal 证据 | 清单必须引用真实执行记录，检查会话/工作区/Goal/尝试身份、真实退出状态及起止/当前快照。新尝试、失败、取消、超时和快照变动使旧成功失效，可逐项重验。 |
| 配置与审批可靠性 | `tool_status` 与 doctor 使用离线投影。Always Allow 锁等待有界可取消，原子替换后目录同步失败返回已提交且持久性未确认的警告，不谎报未生效。 |

接口、配置及完整限制见 [Runtime 工具指南](../../P1_RUNTIME_TOOLS.md)、[决策](../../decisions/2026-11-17-p1-runtime-tools.md)与[架构](../../ARCHITECTURE.md)。

## 已执行的检查与首次失败

| 检查 | 实际结果 |
| --- | --- |
| 生产 core 编译 | `cargo check -p willdeep-core --lib --locked` 通过；早于 TypeScript 兼容调整。 |
| core 首轮全回归 | 单元 1243 通过、1 失败、7 忽略。失败是检查点夹具以新 Goal ID 期待旧目标预算合并；已调整为同 ID 陈旧快照，并新增同名新 ID 不继承预算的断言，下一轮已通过。 |
| core 首轮集成 | AST 2、浏览器 4、Goal 13、模型策略 16、Provider 协议 5、搜索 12 项通过；真实工具入口保留显式忽略，另行运行。 |
| TypeScript 整合后 core 全套 | 单元 1246 通过、2 失败、7 忽略；普通集成全部通过。两项失败来自浏览器进程树夹具：PID 文件已创建但尚未写入，读取空字符串解析失败。夹具改为同目录原子发布完整 PID，最终全套已通过。 |
| 严格 Clippy 首轮 | macOS 报搜索夹具未处理读取长度和并行 TUI 两处无效 clone；Windows 另外报两处仅 Unix 使用的 `mut`。搜索断言读取字节非空，目录构造器只在 Unix 绑定可变对象；TUI 交原会话修正。 |
| Windows 交叉 Clippy 复测 | `x86_64-pc-windows-gnu` 的 core 全目标严格检查通过；仅交叉编译/lint，不代表原生执行。 |
| CLI 编译和首批回归 | no-run 通过；`policy` 27、`local_provider_discovery` 15、`image_download_tests` 2、`config::` 58、`doctor::` 4 项均通过，按完整测试名去重为 96 项。 |
| 真实 Rust 浏览器宿主 | 3 项全部通过：共享 execute 的表单/审批/ref/截图/路由/来源拒绝、审批中取消、恶意 DOM 与只读关闭。 |
| 真实 TypeScript 初轮与复测 | 初轮查询 2 项通过、重命名 1 项失败：服务 5.3.0 固定返回无版本 `changes`，严格宿主拒绝。接入仅 TypeScript 的请求前快照绑定后，实际服务 3 项全部通过。 |
| 浏览器桥首轮 | Chrome 五秒启动窗口超时，未进入功能用例；保留[失败 JSON](browser-bridge.json)和[失败 Markdown](browser-bridge.md)。 |
| 浏览器桥复测 | 11 项通过，见[复测 JSON](browser-bridge-retry.json)及[复测 Markdown](browser-bridge-retry.md)。 |

集成过程中还出现过 Always Allow 测试接口未适配导致的编译错误，以及 TypeScript 文件正在变更时的编译错误；这些批次不算通过。旧停止的修复 Workflow 和重复浏览器 writer 结果不作为验收依据，整合后以本报告最终命令为准。

## 真实浏览器覆盖

Ruby 脚本使用 Node 24.8.0、Chrome 157.0.8103.79 和 `127.0.0.1:19851` 合成服务，不附着日常浏览器。复测覆盖：隔离 profile 与干净环境、打开及快照、真实表单提交和业务断言、PNG 哈希/MIME/尺寸、路由变化与陈旧代次、来源拒绝和非法动作、页面默认执行环境伪造 DOM 不得重定向点击、应用错误页、关闭清理、取消清理所属进程组、挂起 Chrome 后约一秒返回 UNKNOWN 且不自动重试。

桥脚本只验证协议字节；Rust 宿主另行验证动作审批、引用绑定及私有路径落盘。截图成功与业务断言通过分别记录。首次启动失败原因未确定，复测成功不抹去失败，也没有据此修改启动期限。

## CLI 回归覆盖

CLI 最终 no-run 通过。第一批依次执行 `policy`、`local_provider_discovery`、`image_download_tests`、`config::`、`doctor::`，分别通过 27、15、2、58、4 项。补充执行 `harness::verification`、`daemon::diff_review::`、`agent_rsi::native::tests`、`agent_registry::tests`、`daemon::child_sessions::`，分别通过 5、27、6、7、88 项，另有一个显式手工诊断入口忽略。全部命令均为 `cargo test -p willdeep --bin willdeep --locked <filter> -- --test-threads=<N>`，完整参数见 JSON。

这两批共 239 次通过执行，按完整 Rust 测试名去重为 224 项；不能把重叠筛选直接相加当成不同用例。这是相关 CLI 二进制单元回归，不宣称整个工作区测试全套通过。范围包括压缩/Worker/辅助/标题/回退、模型库和移动端缓存、恢复与 worktree 持久归属、启用策略时零探测、无策略兼容、图片非 2xx 与跳转零落盘、生产 Git 证据快照及子会话恢复。

## 工程检查与复现入口

core 全套命令：

```bash
cargo test -p willdeep-core --lib --tests --locked --no-fail-fast
```

实际 TypeScript 服务的三个显式用例：

```bash
cargo test -p willdeep-core --test lsp_tools --locked -- --ignored --test-threads=1
```

全工作区严格 Clippy：

```bash
cargo clippy --workspace --all-targets --locked -- -D warnings
```

Windows GNU 目标的 core 交叉检查：

```bash
cargo clippy -p willdeep-core --all-targets --target x86_64-pc-windows-gnu --locked -- -D warnings
```

版本一致性：

```bash
ruby -E UTF-8:UTF-8 scripts/check_release_version.rb 0.151.0-rc1
```

Ruby 浏览器桥脚本接受已存在的 Node/Chrome 绝对路径，并要求本地合成服务；实际参数、报告前缀和运行边界见 JSON 及桥复测报告。它不会自动安装依赖，也不替代 Rust 宿主验收。

core 最终全回归通过：1248 项单元通过、0 失败、7 忽略；普通集成共 52 项通过、0 失败、7 忽略。普通集成分别为 AST 2、浏览器 4、Goal 13、模型策略 16、Provider 协议 5、搜索 12；LSP 的三个真实用例在显式批次全部通过，浏览器的三个真实宿主用例也已另外执行通过。单元全套耗时 420.69 秒；没有跳过失败项换取通过。

全工作区格式 `cargo fmt --all -- --check` 已通过。Windows GNU 目标 core 严格 Clippy、版本一致、桥/夹具脚本语法、`git diff --check` 和源码篇幅均通过；1644 个手写文件无超过 3000 行的违例，较大文件的职责评估清单保留在[源码篇幅 Markdown](source-size.md)与[JSON](source-size.json)。十份文档的相对链接已检查通过。

全工作区 `cargo clippy --workspace --all-targets --locked -- -D warnings` 最终通过。上述结果均对应实际结束且退出码为零的命令；没有将中途编译失败、静态复核或不同源码阶段的通过项拼成最终 core 全套通过。并行 TUI 测试的两处 clone 和格式修正由原会话完成，已由本会话检查源码和最终门禁，未覆盖其业务实现。

## 未验证或明确不提供的范围

- 本机缺少 `ast-grep`，没有执行真实 AST CLI；离线协议、快照与权限回归不能代替真实工具验收。
- `rust-analyzer --version` 经 rustup 代理返回实际服务二进制不可用，未执行真实 Rust 语言服务。未执行 Windows 原生进程 Job 或原生截图；非 Unix 当前明确拒绝截图落盘。
- 浏览器只供父 Registry，要求有效 OS 围栏 Off；来源拦截、CSP 和 Chrome flags 不构成 Chrome 全进程网络隔离。
- 项目模型策略限制已接入的 Provider HTTP 出口，不约束任意外部进程或插件自行联网。扩大策略须宿主审查并重启 Runtime。
- Goal 证据在进程内保留，重启需重验；生产快照复用 Git Diff 内容指纹，不宣称 SHA-256/CAS，不覆盖 ignored 文件或外部服务。退出码记录不证明任意项目脚本包含足够断言。
- PostTool Hook、结果投递和进程退出仍可能在实际副作用后丢失回执；UNKNOWN 不自动重放。
- 没有真实模型成绩、Token 或成本数据，JSON 对应值保留 `null`，不能按零或成功填写。
