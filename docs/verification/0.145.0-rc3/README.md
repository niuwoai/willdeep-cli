# 0.145.0-rc3 四项整合与发布验收

日期：2026-11-06。基线：`ea6f38baf94c00e2739f79695c16112e865de290`（当前 develop/main，0.145.0-rc2）。整合分支：`fix/combined-session-release-20261106`。

## 四项来源与实际合入状态

| 会话成果 | 来源 | 本次核对 |
| --- | --- | --- |
| 完整子会话 P2 | `feat/child-sessions-p2`；整合提交 `a126369e` | 已存在于基线。协议、接管/脱离、三档订阅、依赖、通知 checkpoint、执行槽与 CLI/TUI/Web 入口及测试均保留。 |
| 原生 Runtime 测试进程回收 | `claude/affectionate-khorana-66872a` 的未提交来源 | 已由 `a126369e` 纳入。私有进程映像、ChildGuard、精确回收与原七项回归均保留；另有 stdin BrokenPipe 展开回收的 Unix 回归。无需复制旧 rc9 元数据。 |
| 聊天内置配置与下一轮生效 | `claude/ecstatic-grothendieck-d9fa27` 的未提交来源 | 来源 40 个路径全部存在，26 个字节相同；差异来自 P2 接线、Windows 修复及版本/文档。三个 compressor 字段、计划/revision、审批、保存、下一轮加载、手动压缩快照及委派边界均保留。 |
| Git 验收快照 | `fix/verification-nested-worktree` 的未提交来源 | rc2 未包含。本轮移植 `diff_review.rs`、`fingerprints.rs` 及其回归、`harness/verification.rs`，补入安全诊断与嵌套仓库指纹，未修改官网忽略规则。 |

独立只读核对覆盖前三项的源码、测试、提交与来源验收边界，不以 CHANGELOG 文字代替代码。相关实现从 `a126369e` 至基线均未被撤回。Git 快照来源开发版本为 0.143.0-rc9，本次版本为 0.145.0-rc3；旧版本验收不算作本次构建通过。

## 验收方法与证据身份

本文件在发版前记录来源与验收方法，不预先宣称执行通过或发布完成。[来源对照](provenance.json)记录实际代码身份；本轮最终 JSON/Markdown 执行结果与安装记录作为 `v0.145.0-rc3` Release 附件交付，不能用历史版本结果替代。

本轮覆盖范围：完整 Rust workspace 回归、严格 Clippy、格式、版本一致性、源码规模、Web 构建与 P2 交互单测；同一固定二进制的 P0/P1/P2、聊天配置及权限组合隔离验收；官网实际工作区的快照诊断。测试代码和产品期限保持不变。

辅助脚本首轮在本机 US-ASCII 默认环境中遇到 Ruby 编码错误，执行器与 Ruby 子命令显式使用 UTF-8 后复跑。该准备失败独立记录，不归入产品测试通过数。

## 安装与发布规则

本机替换前备份 Cargo 与 Homebrew 原二进制，优先安装 Cargo 版并平滑升级 Runtime，发行产物就绪后再更新 Homebrew。公开发布通过 feature 分支与 PR 合入默认分支后创建新 tag，不移动 rc2 tag。macOS 发行产物继续使用 universal。实际安装版本、备份位置、CI 和发布结果以 Release 附件为准。

## 边界与历史限制

历史 rc1 高负载 Runtime 与 64 Worker 宿主取消超时记录仍保留，不因来源已合入而宣称已修复。Linux/Windows 原生行为以本版 CI 为准；本机交叉 Clippy 不能代替原生执行。隔离 Provider 使用回环夹具，不发送远端付费模型请求；真实工作区诊断不读取或输出文件正文、配置与凭据。
