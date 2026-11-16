# 0.149.0-rc12 验收记录

状态：本地验收通过，跨平台 CI 与公开交付待执行。本文与 JSON 记录实际执行结果，不把本地通过等同于发布完成。

## 本次修正

保留原验证门禁修复和既有 TUI/Runtime 交互。rc11 跨平台验收又发现基线缺陷，本候选补充：

- Runtime 创建心跳后的正常返回及可返回错误统一执行取消、等待同一心跳任务结束、按归属清理状态，避免同步原子写在清理后重新创建旧租约。
- 插件安装来源、已有副本、staging、内容审批和启用检查读取当前字节。路径、大小和修改时间相同不再替代权威内容校验；只读展示保留明确的缓存预览入口，fresh 与 cached 共用文件筛选、排序、限制和 SHA 编码。新增 12 项确定性回归，两项独立静态审查未发现具体缺陷；静态核对不代替测试。

## 最终固定源码验收

最终任务 `b7k90rbyt` 退出码 0。验收从已提交的 rc11 HEAD `25415efc846b0c7afd5a1a9f11b785b7bea5a46e` 加本候选未提交改动开始；前后 HEAD 与生产源码摘要相同，摘要为 `098806f6f6b55cd64f4dc33f0ab1cec57d791d6940a15fa556944b21d9b5f025`。源码摘要覆盖 `crates`、`web/src`、`Cargo.toml`、`Cargo.lock` 的 Git 已跟踪及非忽略新文件内容。正式产物须从最终干净发布提交重新构建。

Rust workspace 2,878 项通过、0 失败、12 项忽略：

| Suite | 通过 | 失败 | 忽略 |
| --- | ---: | ---: | ---: |
| CLI unit | 1,637 | 0 | 5 |
| Detached jobs integration | 18 | 0 | 1 |
| Headless Runtime integration | 62 | 0 | 0 |
| Core unit | 1,091 | 0 | 6 |
| Provider contracts | 5 | 0 | 0 |
| Runtime client unit | 7 | 0 | 0 |
| Runtime protocol unit | 57 | 0 | 0 |
| Runtime client doctest | 1 | 0 | 0 |

两次嵌套子进程 helper 的 `1 passed` 不重复计入总数；下面的定点重跑也不重复计入 workspace 总数。核心 suite 耗时 420.66 秒，包含目标循环的分钟级真实退避与 4,114 条恢复记录的落盘分页检查；用例最终均通过，没有缩短等待或放宽断言。

| 检查 | 实际结果 |
| --- | --- |
| 插件 core 定点 | 67 项通过 |
| 插件 CLI 定点 | 7 项通过 |
| Runtime 关闭定点 | 5 项通过 |
| 进程清理边界 | 17 项通过 |
| 原 rewind 回归 | 1 项通过 |
| 严格 Clippy | 全工作区、all-targets、`-D warnings`，退出码 0，63 秒 |
| rustfmt | 全工作区通过 |
| 源码长度 | 1,528 文件，0 违规，上限 3,000 行 |
| 后台合同 | 与 Xedit 合同一致，通过 |
| 版本一致与差异空白 | 通过 |
| Web Vitest | 52 文件、430 项通过，两个 worker |
| TypeScript 与 Web 生产构建 | 通过；构建保留既有分块大小提示 |
| ESLint | 0 错误、1 项既有 ModelPicker Hook 警告 |
| Ruby CI 守卫 | 71 runs、1,072 assertions，0 失败/错误/跳过 |
| npm 安装合同 | 7 runs、38 assertions，0 失败/错误/跳过 |
| 配置桥接 | 6 项通过 |
| Magpie 插件合同 | 19 项通过 |
| 模型评估任务定义 | 20 个、0 不合格 |
| CI shell/YAML 示例 | 通过 |
| npm pack dry-run | 6 个发布文件，通过 |
| 真实 TUI（rc12 Debug） | 3 种布局、22 次宽度检查通过；隔离目录，2 次本机模拟请求 |

Web 源码与相关检查通过后保持不变。真实 TUI 证据保存在 `.claude/scratch/rc12-tui-debug/`；SVG 由实际 ANSI 单元格还原，属于终端渲染证据，不是系统截图。未读取真实 Provider 凭据或请求真实模型。

浏览器分享页预览受工具配置阻挡：`preview_start` 未识别 CLI 工作树已有的 `cli-share-release-preview`，返回的启动项属于 Xedit；切换会话工作目录后再次调用仍相同。未修改其他项目配置，未将本次浏览器验收记为通过。

## 已停止的首批 Rust 验证

关闭确定性回归 5 项、进程清理边界 17 项、原 rewind 用例 1 项通过。之后 workspace 批次中的 CLI 1,634 项通过、5 项忽略，detached 18 项通过、1 项忽略。为纳入随后确认的插件摘要修正，主线程主动停止后续 headless 批次；该批次全量与严格 Clippy 不记为通过。注入 heartbeat panic、BrokenPipe 和清理 panic 是测试输入，相关用例按预期通过。

## 真实失败与修正依据

- iMac 的旧候选 head `25415efc`：CLI 1,629 通过、detached 18 通过，headless 61 通过/1 失败。原 rewind 在 10 秒清理截止后失败，日志反映的是多次枚举累计时间。之后定点复验通过，不能替代根修正。代码的取消/清理写回竞态可用受控 writer gate 确定重现，但原失败瞬间未完整取证，保留该边界。
- q4 的同一旧 head：代理修复后越过下载阶段，CLI 1,626 通过/1 失败/5 忽略。失败为同版本插件内容变化未被拒绝。现场 tmpfs 上来源 HTML 为 `two`、已安装副本为 `one`，长度相同，清单和 payload 的 mtime、ctime 全部相同，与元数据缓存误命中路径一致。
- 首次源码行数检查受 US-ASCII locale 影响，UTF-8 locale 下复验通过；首次 npm `--prefix` 调用未正确选中 pack 目录，显式包路径后通过。这两项没有修改生产源码。

## 交付状态与限制

本记录生成时尚未发布 rc12；Cargo 与 Runtime 为 rc11，Homebrew 为 rc1。当前 rc11 程序与其独立备份的 SHA-256 已复核一致，旧 rc10、rc1 和会话热备继续保留。会话热备不宣称停机一致恢复包。跨平台 CI、默认分支集成、正式产物、tag、GitHub Release、npm、Homebrew 与本机更新的实际结果另行记录。

插件新鲜摘要修正不等于目录原子快照，也不宣称解决所有运行中修改和 TOCTOU。Runtime 修正覆盖普通返回与 Result 早退，不宣称覆盖外部取消、主体 panic 或强杀。真实用户会话未修改，Provider 退避状态行仍为独立待办。
