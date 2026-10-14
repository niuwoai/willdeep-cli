# rc10 命令超时资源释放修复

日期：2026-10-14。基线：`318ae37fac349bfe395b4dcf595532bb79f683a7`，保留 rc8 运行机制修复与 rc9 Windows 账本文件 ID 修复。

## 原始故障

rc9 Windows 原生 CI 中，`deadline_prevents_expired_commands_and_bounds_running_commands` 的截止与错误类型断言通过，但立即删除工作目录时遇到错误 32。超时返回路径依赖 drop 触发终止，没有等待进程退出完成，工作目录可能仍被占用。原失败日志由发布流程保留。

## 修复与边界

- `run_capture_logged` 超时或捕获出错后显式终止进程组，再等待主进程退出；清理最多等待 1 秒。
- Windows 使用 `TerminateJobObject`，保持 Job 句柄并查询 `ActiveProcesses`，等待全部成员归零后释放句柄。正常主进程退出也等待成员清理完成。
- 清理失败会追加明确错误上下文；不宣称资源已经释放。执行超时仍保持原 `TimedOut` 类型与输出尾部。
- future 被取消时无法继续异步等待，保留原有 Job kill-on-close、子进程 kill-on-drop 和 Unix 进程组 Drop 兜底。
- 保留原 50 毫秒期限、2 秒外部截止断言；Windows 新测试在返回后直接删除目录，不加睡眠或删除重试，再验证子进程没有延后写入。
- 发布工作流复用完整 CI，公开 Release 必须等待各平台构建和 CI 同时成功。

## 验证材料

- 本地执行专项 12 项通过，包含主进程已回收、延后写入阻止、日志尾部与取消回归。
- Windows GNU 条件编译检查通过；这不是 Windows 原生运行验证。
- 工作流 YAML 解析和 `actionlint` 通过。
- Windows GNU all-targets Clippy（`-D warnings`）通过；同步终止方法仍被流式命令执行调用，保留该接口。
- 完整工程 7 项门禁全部通过：格式、工作区 all-targets Clippy、工作区测试、Web lint/build、文件规模和 diff 检查。Core 为 737 通过、0 失败、6 忽略；CLI 为 685 通过、0 失败、1 忽略；CLI/Runtime 集成组分别 18/30 通过。原有忽略项未改变。
- 完整工程命令、退出码、耗时与各组测试摘要详见同目录 JSON；Windows 原生 CI 由发布流程执行后再确认。

本次不产生新的 SWE-bench 模型成绩，不更新历史基准结果，不证明模型语义错误已经解决。
