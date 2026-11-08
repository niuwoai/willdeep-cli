# 0.147.0-rc2 本地验收

日期：2026-11-06。范围：Provider 思考等级映射、TUI/Web 会话选择、单张当前计划卡。

状态：Core/CLI/Web 定向测试、构建和隔离界面复验通过，已安装本机 Cargo 体验版。真实 Runtime 仍为 `0.145.0-rc3`，存在运行任务和等待审批，尚未执行升级。另在诊断真实客户端的隐藏审批与发送等待问题；本记录的界面通过只覆盖思考等级和计划卡。公开发版仍须用户明确确认。

## 验收契约

- Provider 可定义逻辑名称、默认等级、协议参数路径，映射值支持字符串、数字、布尔值。
- TUI `/reasoning` 查询，`/reasoning <等级>` 设置，`/reasoning default` 清除；`/reasoning set <等级>` 可选择与命令重名的合法等级。
- 会话等级保存在 Runtime 元数据中，尚未发送首条消息的会话也可恢复；不写进可导出的 Core 会话。
- 切换成功前先校验并持久化；已领取任务保留原快照，后续领取的任务读新等级。无效输入或持久化失败不覆盖已接受的选择。
- 配置页只修改思考映射，保留隐藏连接字段、环境变量引用和协议推断；兼容内联 TOML、特殊合法名称和显式清空。
- 计划更新与替换复用当前卡片。步骤结构变化清除无关旧详情；仅进度变化保留详情，实时显示与历史回放一致。

## 已执行检查

| 检查 | 结果 | 说明 |
|---|---|---|
| Core 定向测试 | 39 通过，0 失败 | 计划投影/回执、逻辑等级校验、三协议默认路径、数字/布尔值、本地 Chat Completions 普通与流式请求 |
| Web 定向 Vitest | 7 通过，0 失败 | 两个文件：映射往返、专用保存、无映射时清除旧覆盖 |
| TypeScript noEmit | 通过 | `tsconfig.app.json` |
| 定向 ESLint | 通过 | 思考等级两个组件及对应测试 |
| Web 生产构建 | 通过 | 存在既有的单个 JS chunk 超过 500 kB 提示 |
| `cargo fmt --check` | 通过 | 格式整理仅针对本轮接线代码 |
| `git diff --check` | 通过 | 无空白错误 |
| CLI 定向测试 | 21 通过，0 失败 | 计划卡 10、TUI 等级恢复/队列快照 6、Provider 配置保存 5 |
| 最终 CLI 构建 | 通过 | `cargo build --locked --offline -p willdeep --bin willdeep` |
| 修复后真实界面复验 | 通过 | TUI 重开恢复 `deep`、两端单张计划卡、Web 保存与选择 |
| 本地 Cargo 安装 | 通过 | `cargo install --path crates/willdeep-cli --locked --offline`，安装后 `--version` 返回 `0.147.0-rc2` |

核心测试命令：

```bash
cargo test --locked --offline -p willdeep-core --lib -- provider::reasoning_profile_tests:: provider::chat_completions::reasoning_level_tests:: provider::chat_completions::generation_limit_tests:: conversation::tests:: planning::tests:: --nocapture
```

CLI 定向测试命令：

```bash
cargo test --locked --offline -p willdeep --bin willdeep -- tui::reasoning_commands::tests:: tui::plan_ui::tests:: web::configuration::tests:: --nocapture
```

Web 定向测试命令：

```bash
yarn --cwd web test src/ConfigurationSettings.reasoning.test.tsx src/ReasoningPicker.test.tsx
```

## 真实运行与隔离边界

- 使用 `/tmp/willdeep-web-rc7` 的隔离 HOME、配置与演示会话，Web 监听 `9851`。
- 未访问真实 `9847` 服务，未修改用户真实 Provider 配置，未向上游模型发送请求。
- Web 已实际操作 `economy → Provider 默认 → deep`，读取会话 API 核对结果；截图显示自定义等级及映射编辑器。
- TUI 通过 tmux 启动真实 CLI 并发送命令；图片由捕获的终端字符与 ANSI 颜色渲染。首次恢复检查发现空会话未读取 Runtime 等级，已修复；最终重新打开会话并执行 `/reasoning`，实际显示 `deep`、默认 `balanced`。
- 配置页面实际将默认值改为 `deep` 保存，再恢复 `balanced` 保存；刷新页面后默认值及全部映射保持。浏览器控制台没有错误，检查时网络缓冲中没有失败请求。
- 计划卡使用隔离的示例会话回放多次 plan/progress 记录，不将演示记录当成真实模型执行结果。
- 截图位于本地忽略目录 `.claude/scratch/reasoning-preview/`，已通过会话文件卡交付 TUI、Web 和配置截图；最终 TUI 图与 Web 选择器图已更新交付。
- 本地安装前已备份 Cargo 入口至 `~/.local/share/willdeep-cli/local-backups/0.145.0-rc4-20261106-225522/willdeep`，SHA256 见 `results.json`。Homebrew 保持 `0.145.0-rc4`。
- 真实 Runtime 经正常 CLI 控制接口确认 `version=0.145.0-rc3`、`pid=30102`、`status=ok`。升级前发现现有运行任务与隐藏审批，故没有执行交接。用户反馈 TUI 没有审批卡后，通过 `interaction.present` 恢复原请求展示，未代替用户作出审批决定；后续状态查询显示原轮次回到 running。
- Runtime 提前收起审批、却在独立消息不可执行时不恢复的问题，在当前 rc2 中仍存在；已单独进入 rc3 修复，不计入 rc2 已通过的功能。
- 安装构建通过，但 Cargo 提示锁定的 `chacha20 0.10.1`、`clipboard-win 5.4.1` 已被上游撤回。本次保持锁文件依赖，不做无关依赖升级。

## 曾出现的失败

- 旧计划测试仍断言两个卡片，得到 `left: 1, right: 2`；已更新为一卡和旧详情清理断言，最终 Core 39 项全部通过。
- CLI 测试构造器缺少新增 `reasoning_level` 字段，已补齐。
- 最终 CLI 首次编译遇到 `E0425/E0422`：`RemoteSessionState` 模块路径不正确；已改为 `daemon::tui_bridge`，重跑 CLI 21 项与构建均通过。

本次未运行全工作区测试、Windows/Linux 原生验证或真实上游兼容性调用。未提交、推送、打 tag 或发布。
