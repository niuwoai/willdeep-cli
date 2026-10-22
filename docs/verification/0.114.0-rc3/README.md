# 0.114.0-rc3：Runtime 交接收尾竞态

日期：2026-10-22。基于 rc2 的流式、模型选择和本地 Provider 发现验收继续验证。

## CI 发现的问题

独立 tag CI `39746216187` 的 Windows `daemon_upgrade_drains_active_work_and_hands_off_without_loss` 失败：活动 Turn 在交接后被恢复为 Interrupted，CLI 退出码 5。rc2 发布流水线 `39746216312` 在公开发行之前取消；保留原 tag，不移动。

`TaskManager::finish` 在同一持久化锁内依次更新任务、Agent、任务事件、会话 Turn 与终态事件；原 `wait_until_idle` 只读取内存任务状态，可能在任务已终态而 Turn 尚未落盘的间隙触发 Runtime 关闭。新的空闲判断也取得该持久化锁，等完整提交后才允许交接。

专项回归显式持有终态提交锁，要求空闲等待保持阻塞；释放锁后必须及时完成。不通过增加固定等待或忽略 Windows 测试规避竞态。

流式修复与实测数据见 [rc2 验收](../0.114.0-rc2/README.md)。本版本重新核验专项测试、交接集成测试、静态检查、实际到屏延迟和公开跨平台发布。

## 本地验证

- 确定性回归：原实现失败，修复后通过。
- CLI 完整单测：847 通过、0 失败、3 项原有忽略项。
- 实际活动轮次升级交接：首轮通过，随后连续重复三轮均通过；原请求完成、旧流关闭、新 Runtime 可继续接收请求。
- workspace / all-target Clippy、cargo fmt、文件尺寸边界、Web 0.114.0-rc3 构建通过。
- 本次修改仅补上空闲判断与已有持久化事务之间的同步边界，未改动 Provider 接收、事件分组提交或 TUI 渲染路径。
