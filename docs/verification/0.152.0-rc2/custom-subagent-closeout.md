# 自定义智能体启动故障：修复与验收

日期：2026-11-17。结果：本机启动已恢复，诊断改进与隔离派工验收通过。

## 原因与本机修复

`[subagents.doc-multilingual]` 被误写入 `~/.willdeep/config.toml`。该配置区只允许覆盖内置工种参数；自定义智能体应由智能体库管理，完整 ID 为 `user/doc-multilingual`。未知工种使整份配置校验失败，因此启动中断。

`Local Provider discovery` 和最终的 `error` 都来自同一配置错误，不能据此判断 Provider 故障。

本机原配置段已完整注释保留。通过 `agents bind` 将原 Provider、模型及 65536 上下文窗口迁移至已有安装项；智能体定义、权限与历史版本保留。已安装 CLI 的 `config check` 通过。

## 验收证据

| 范围 | 结果 | 证据 |
| --- | --- | --- |
| 真实配置与已安装 CLI 0.149.0-rc12 | 欢迎界面、`/version` 交互、退出回收均通过 | [真实配置验收 JSON](custom-subagent-installed-smoke.json) |
| 配置、智能体库及语言资源原生回归 | 61 + 33 + 1，共 95 项通过 | 本会话 Cargo 测试日志 `bvk6p78e2.output` |
| 新增的三个隔离回归 | 三语言诊断、全部内置/兼容工种、自定义 binding 加载通过 | `config::subagent_tests` |
| debug 构建 | 通过，产物版本 0.152.0-rc2 | 隔离验收报告记录版本及 SHA-256 |
| 真实 PTY 与 `/agents` | 欢迎界面和已安装自定义指令均可见 | [PTY 原始输出](custom-subagent-startup.ansi) |
| `run --local` | 自定义指令、独立模型、结果交回父会话均通过 | [隔离验收 JSON](custom-subagent-startup.json) |
| 默认 Runtime `run` | 自定义指令、独立模型、结果交回父会话均通过 | 同一隔离验收 JSON |
| 版本一致性、定向格式、差异空白、源码长度 | 通过 | 本会话工程检查日志 |

隔离派工仅访问回环模型夹具，使用临时 HOME、配置和智能体库，不消耗真实外部模型服务。成功夹具已清理，私有 Daemon 已停止。

验收脚本：[verify_custom_subagent_startup.rb](../../../scripts/verify_custom_subagent_startup.rb)。运行方式：

```bash
ruby scripts/verify_custom_subagent_startup.rb target/debug/willdeep docs/verification/0.152.0-rc2
```

## 失败记录与修正

[隔离验收 JSON](custom-subagent-startup.json) 的 `previous_attempts` 保留了此前失败。首轮 `/agents` 的 ID 滚到可见区域之外，原始 PTY 输出没有绘制该行，断言改查实际可见的自定义指令。

另有 PTY 清理失败：macOS 上未读终端输出使子进程停留在退出阶段，阻塞等待或只发 TERM/KILL 无法完成回收。夹具改为先关闭两端 PTY 句柄，再按截止时间非阻塞回收；已在真实 CLI 与隔离验收中验证。曾中止的验收进程已停止，失败夹具保留供核对。

## 代码交付边界

0.152.0-rc2 保留严格工种校验，并增加配置路径、具体段名及中英日修复提示；文档明确区分内置工种覆盖与自定义智能体 binding。同步了版本文件、CHANGELOG 和产品概览。

本会话未提交、推送或发布，未替换 PATH 中的已安装 CLI。真实配置的启动恢复已在原有 0.149.0-rc12 上验证；新诊断改进位于当前工作树中。此报告只覆盖上述范围，整合会话负责的全工程检查不在本报告中推断为通过。
