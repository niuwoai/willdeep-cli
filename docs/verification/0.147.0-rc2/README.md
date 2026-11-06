# 0.147.0-rc2 思考等级与计划卡验收

历史验证日期：2026-11-06。记录来源：`f246a18e935dd80bc91edebd3e2dae6e8c1898b5` 中保存的 rc2 验收资料。范围同时覆盖 Provider 思考等级映射、TUI/Web 会话选择和单张当前计划卡；拆分后前两者属于前置 rc1，计划卡属于 rc2。

本记录中的通过数和交互观察均为拆分前历史结果，不是本次提交的重新验证；新增跨 Provider 继承、选择器刷新回归以及 Web 模块拆分、Harness 接线与移动端弹层调整不在这些历史结果中。本次拆分未编译、运行测试、安装或发布，安装现场信息不作为验收证据。

## 验收契约

- Provider 可定义逻辑等级、默认等级和请求参数路径，映射值支持字符串、数字、布尔值。
- `/reasoning` 查询，`/reasoning <等级>` 设置，`default` 清除；`set <等级>` 支持与子命令重名的合法等级。
- 会话覆盖保存在 Runtime 元数据中，空会话也能恢复，不写入可导出的 Core 会话。
- 切换成功前先校验并持久化；已领取任务保留快照，后续任务读取新等级。无效值与保存失败不覆盖已有选择。
- 配置专用保存保留连接字段、环境变量引用和协议设置；覆盖内联 TOML、特殊合法名称、显式清空及已有免凭据 Provider。
- 实时与历史回放共用一张计划卡；结构变化清除旧详情，状态更新保留展开状态。

## 历史检查记录

| 检查 | 结果 |
|---|---|
| Core 定向测试 | 39 通过：计划投影、等级校验、协议默认路径、标量映射、本地 Chat Completions 普通与流式请求 |
| CLI 定向测试 | 21 通过：计划卡 10、等级恢复与快照 6、配置保存 5 |
| Web 定向测试 | 7 通过，2 个测试文件 |
| TypeScript、定向 ESLint、Web 构建 | 通过；保留既有大 chunk 提示 |
| cargo fmt、git diff --check、CLI 构建 | 通过 |
| 真实 TUI | 重开空会话后 `/reasoning` 显示 `deep`，默认 `balanced`；计划卡只有一张 |
| 真实 Web | `economy → 默认 → deep` 与会话 API 一致；多次计划更新只有一张卡 |
| 配置界面 | 默认等级 `deep → balanced` 保存成功，刷新后映射保持 |

```bash
cargo test --locked --offline -p willdeep-core --lib -- provider::reasoning_profile_tests:: provider::chat_completions::reasoning_level_tests:: provider::chat_completions::generation_limit_tests:: conversation::tests:: planning::tests:: --nocapture
```

```bash
cargo test --locked --offline -p willdeep --bin willdeep -- tui::reasoning_commands::tests:: tui::plan_ui::tests:: web::configuration::tests:: --nocapture
```

```bash
yarn --cwd web test src/ConfigurationSettings.reasoning.test.tsx src/ReasoningPicker.test.tsx
```

## 证据边界

真实界面验收使用独立 HOME、合成会话和本地配置；未修改真实 Provider 配置、未向真实上游发送请求。计划卡回放是确定性夹具，不代表真实模型完成了对应工作。

曾遇到旧计划测试仍断言两张卡、测试构造器缺少 `reasoning_level` 和 `RemoteSessionState` 路径错误；修复后上述定向测试通过。Cargo 保留锁文件中的 `chacha20 0.10.1`、`clipboard-win 5.4.1`，未顺带升级依赖。

未运行全工作区、Windows/Linux 原生、真实上游兼容性或回退演练。此阶段未公开发布；记录范围不包含审批可见性恢复与忙时输入反馈。
