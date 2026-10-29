# 会话计划依赖与并行状态验收

- 日期：2026-10-29
- 版本：0.129.0-rc1
- 分支：feature/parallel-plan-dependencies
- 范围：依赖图校验、Agent 计划工具与调度提示、会话恢复、TUI/Web 计划卡片和实时事件。

## 行为

普通会话与目标模式都可使用 `update_plan`。步骤通过稳定 ID 和 `depends_on` 表达前置关系；成功更新返回 `ready`、`running`、`waiting`。Agent 根据这些状态调用既有后台 Worker、后台命令和只读并行工具，在权限、预算与共享资源约束内推进独立步骤。

依赖图拒绝未知 ID、自依赖、重复依赖与循环。前置步骤必须为 `done` 才允许后续步骤进入 `in_progress` 或 `done`；失败、阻塞、跳过均不自动满足依赖。`merge=true` 局部更新保留其他步骤的运行状态与依赖。相同计划的完整快照原位更新卡片。

计划回执进入历史、实时事件与会话检查点。恢复后重新注入当前计划，压缩后的对话仍能保留依赖及多个运行中步骤。TUI/Web 显示并行运行数量和等待的前置步骤。目标模式仍要求验收证据，受限制 Worker 不会因此获得主 Agent 的计划工具。

这是 Agent 使用的依赖校验与就绪步骤计算，不是独立于 Agent 自动执行任意命令的作业调度器。步骤状态由 Agent 根据执行结果更新；计划状态本身不作为完成证据。恢复后的运行中步骤需核对已有任务，不自动重新发起副作用。

## 检查结果

| 检查 | 结果 |
| --- | --- |
| `cargo test -p willdeep-core plan --lib -- --test-threads=2` | 15 通过 |
| 同次编译的 core 测试二进制，精确执行 `agent::tests::a_goal_with_criteria_completes_only_after_evidence_is_recorded` | 1 通过；完成仍须提供验收证据 |
| `cargo test -p willdeep plan --bin willdeep` | 7 通过，含 TUI 多项进行中、历史回放与 SSE 计划回执 |
| `yarn test src/ConversationCard.test.tsx` | 3 通过 |
| `yarn test --maxWorkers=1` | 182 通过、1 个既有失败 |
| `yarn build` | 通过，仍有主包超过 500 kB 的构建提示 |
| 本次涉及 Rust 文件的 `rustfmt --check`；`git diff --check` | 通过 |
| Cargo、Web、npm、安装清单与 CHANGELOG 版本一致性 | 通过 |

Web 唯一失败为 `ProviderSetup.test.tsx` 的旧文案断言：期待审核插件权限，而现有实现文案为直接填写模型信息。已对比 HEAD，组件、测试与相关设置页文案均未被本次修改。默认并发的全量 Web 测试曾出现超时；降低至一个 Worker 后超时未复现。

全量 core 测试曾运行，但无关的 `recovery_inventory_beyond_4096_paginates_within_parent_and_marks_reports` 长时间未结束，因此停止该轮全量测试，以上述针对性回归为本次验收依据；不宣称全量 Rust 测试通过。

验收使用模拟 Provider、实际计划工具/检查点、组件及事件投影测试；没有额外调用付费模型或真实发布流水线。共享工作区中的模型评估和另一组回复风格改动已保留。未提交、发布、安装或重启现有 Runtime。
