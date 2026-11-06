# 0.147.0-rc3 审批恢复与忙时反馈验收

历史验证日期：2026-11-07。记录来源：`f246a18e935dd80bc91edebd3e2dae6e8c1898b5` 中保存的 rc3 验收资料，保留审批恢复、忙时反馈的历史定向测试与隔离环境观察。

本记录不是本次拆分提交的重新验证；新增跨 Provider 继承、选择器刷新回归以及 Web 模块拆分、Harness 接线与移动端弹层调整不在这些历史结果中。本次拆分未编译、运行测试、安装或发布。安装和 Runtime 交接现场已移除，不以这些历史操作证明当前提交可部署或任务可无损恢复。

## 问题与修复契约

旧调度器在独立性判断之前收起审批。所有候选都依赖原任务或判断失败时，没有独立任务启动，也没有完成事件恢复审批：原轮次等审批、后续轮次排队，界面却隐藏了等待项。旧 rc2 已在隔离 Runtime 中复现。

- 候选通过独立性判断、在锁内复核会话与消息快照并领取成功后，才收起原审批。
- 候选被拒、判断未知、分类或领取失败时，保持审批可见；无独立任务执行时恢复遗留的隐藏请求，包括空队列。
- 保留原交互身份、等待 sender、权限与消息；展示恢复不表示审批通过。
- 真正独立任务执行期间继续收起原请求，结束后恢复；不绕过停止抑制和升级排空。
- 单会话错误不阻止其他会话恢复；撤销独立确认后旧快照不能继续领取。
- TUI/Web 忙时即时显示已有工具进展或等待原因，无证据时说明尚未收到新进展。
- 等待审批或回答时，普通文本 Enter 保留输入、附件并打开原请求；Cmd/Ctrl+Enter 明确入队。成功反馈区分持久保存与模型已读取，失败保留草稿和重试请求标识。

普通 Enter 仍使用持久 Turn 队列。现有 `turn.steer` 是内存收件箱，缺少完整幂等交接和崩溃恢复；本轮没有将它接回默认发送。即时状态反馈不等于模型立即回答任意问题。

## 历史验证记录

| 检查 | 结果 |
|---|---|
| 工作区、内部依赖、Web/npm、安装清单版本 | 统一为 0.147.0-rc3 |
| Rust 定向单元测试 | 52/52：审批调度 18、Runtime 队列 13、思考等级 6、计划卡 10、Web 配置 5 |
| Runtime 集成测试 | 1/1：越过依赖队首执行独立任务，再恢复原问题 |
| Web 交互测试 | 31/31：App 17、队列文案 5、人工等待卡 5、队列组件 4 |
| TypeScript、Web 构建、定向 ESLint | 通过；保留既有大 chunk 提示 |
| cargo fmt、git diff --check、Debug/Release 构建 | 通过 |
| 旧 rc2 隔离复现 | 请求与消息仍在，但审批可见性断言失败 |
| rc3 隔离 Runtime | 请求保留、审批可见、依赖消息排队、原请求未被回答，四项均通过 |
| TUI 历史截图 | 原审批可见，截图显示待发 1；未提供提交后的对照帧 |
| Web 历史截图 | 原审批可见、队列为 1，输入区有草稿；单张截图不证明 Enter 或 Cmd+Enter 的完整过程 |
| 浏览器历史检查 | 检查时控制台和失败请求缓冲区没有错误 |

```bash
cargo test --locked --offline -p willdeep --bin willdeep -- daemon::deferred_gates::tests:: tui::runtime_queue::tests:: tui::reasoning_commands::tests:: tui::plan_ui::tests:: web::configuration::tests:: --nocapture
```

```bash
cargo test --locked --offline -p willdeep --test headless_runtime runtime_yields_past_a_dependent_head_then_restores_the_original_question -- --nocapture
```

```bash
yarn --cwd web test src/App.workspaceApproval.test.tsx src/queueMessages.test.ts src/RuntimeHumanGates.test.tsx src/RuntimeQueue.test.tsx
```

首轮 Rust 测试因测试误用未依赖的 `tempfile` 无法编译，改用唯一隔离路径后通过。Web 附件夹具缺少 `clipboard.files`，补齐后 App 的 17 项复跑通过，其余 14 项此前通过。ESLint 首次目录错误，修正后通过。

思考等级与单张计划卡完整实现的历史验收见 [rc2 记录](../0.147.0-rc2/README.md)。原 rc3 验收执行了相应 Rust 接线测试，没有把历史 Core 测试计入 52 项；该数字不包含本次拆分后的复验。

## 截图与范围

- [TUI 审批卡与待发队列](tui-approval.png)：历史 tmux 字符帧和 ANSI 颜色渲染，待发数为 1。
- [Web 审批卡与待发队列](web-approval.jpg)：历史浏览器截图，待发数为 1。

两张截图只证明各自时刻的审批可见性、队列和草稿状态，不支持队列由 1 变为 2、跨端同步成功或提交后清空草稿的完整操作断言；这些历史交互结论不计作截图验收通过。

真实交互验收使用独立 HOME、临时工作区和确定性的本地 Provider；没有使用真实凭据或向真实上游发送测试请求。更早网络轮询可能被浏览器缓冲淘汰，因此浏览器检查不是完整网络归档。

审批恢复测试不代表强制停止或升级会保留所有请求的待执行状态，部署和恢复兼容性需要单独验证。

未运行全工作区、Windows/Linux 原生、真实上游兼容性或降级恢复演练。Cargo 保留锁文件中被上游 yank 的 `chacha20 0.10.1` 和 `clipboard-win 5.4.1`，未顺带升级依赖。本次无新增数据库迁移，尚未公开发布。
