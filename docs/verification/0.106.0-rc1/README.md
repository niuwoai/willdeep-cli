# 确认菜单、队列鼠标与多 Provider 验收

日期：2026-10-20。源码版本：0.106.0-rc1。以下为发布前源码与隔离运行验收，公开产物、安装和运行中 Runtime 另行读回确认。

## 行为

- 输入区仅输入 `#1983#` 后回车，打开本地确认示例。Y / Enter 允许，N / Esc 拒绝，结果显示在状态栏。
- 示例不提交模型或 Runtime 请求，不插入聊天历史，不改变当前任务或队列，并保留草稿附件。真实审批或提问已经打开时，不替换它。
- 审批选项各占一行，相邻选项留一行空白；空白不可点击。极小终端按空间缩减间隔。
- 模型、会话、工作区、命令、媒体列表保持原布局。
- Ctrl+Q 队列支持点击消息选择；底部固定提供发送、编辑、附件、删除、关闭及快捷键。编辑时切换为保存和取消编辑，窄窗口自动换行。发送复用原有的接收确认流程，失败保留消息；已送达的消息不可撤回。

## 验证

- `cargo test -p willdeep --bin willdeep tui:: --offline`：250 通过；队列布局最后调整后，队列专项 19 项重新通过。
- `cargo test -p willdeep --test workspace_versions --offline`：1 通过。
- `cargo clippy -p willdeep --bin willdeep --offline -- -D warnings`：通过。
- `cargo fmt --all -- --check`、`git diff --check`、源码行数检查：通过。
- 使用独立临时配置和不可用的本机测试 Provider，真实 PTY 中输入触发词并回车，弹出确认示例；Y 在第 15 行，N 在第 17 行，第 16 行为空白；分别选择 N 和 Y，观察到已拒绝和已允许的状态反馈。
- 单元测试覆盖运行中打开示例、附件保留、不排队、不替换真实审批、鼠标点击选项、空白不响应、小窗口布局。
- 队列新增测试覆盖鼠标选择、发送前保留消息、删除所选项、编辑保存与取消、空队列、窄窗口换行和按钮间空白不响应。
- 隔离模拟 Provider 的真实 PTY 验收：任务运行中加入两条消息，Ctrl+Q 后用鼠标点击删除第一条、发送第二条。屏幕显示第二条已送达且不可撤回；落盘会话确认第二条已读取，第一条未进入模型历史，并收到助手完成回复。测试服务已清理，用户原 Runtime 和会话未参与此测试。

Runtime 的 rc4 升级验收与此开发版本分开：已安装 Cargo/Homebrew CLI、运行中的 Runtime、Web 健康响应均为 0.105.0-rc4。旧轮次在交接期间中断，随后在原会话接续；备份中 1318 条消息除重新生成的系统提示外均逐条保留，原插话保留一份，接续轮次已读取监控事件并开始回复。该核验不代表远端评测已经全部完成。

## `/model` Provider 选择

- 顶部显示当前 Provider 和正在浏览的 Provider。Ctrl+P 或点击顶部打开 Provider 列表；方向键、分页、回车和鼠标点击均可操作。
- 选择 Provider 只加载它的模型列表，选择模型后才统一应用。关闭浏览不切换连接；同一 Provider 内切模型保留启动时的连接覆盖。
- 运行中记录 Provider 和模型组合，本轮结束、排队消息发出之前应用。切换会话时清除旧会话待应用的选择。
- 默认子 Agent 跟随完整连接；仅固定模型的子 Agent 保留模型、继承新连接；显式绑定其他 Provider 的子 Agent 保持绑定。离开 some.im 后，其隐式托管工种不再继续请求旧端点或托管模型。
- Runtime 的 `session.configure` 在空闲时同步 Core 与 Runtime 会话的 Provider / 模型，并在 Core 保存失败时回滚 Runtime 元数据；运行中的移动端配置仍只作用于下一轮，保留当前执行所有权。
- 每次列表加载使用独立请求标识，旧响应不会覆盖新 Provider 的模型或 Provider 列表；列表不支持或失败时，可直接输入模型名回车。

最终 CLI 回归为 **768 通过、1 忽略**；Core 主模型句柄测试 **3 通过**；最后的运行中配置保护调整后，移动网关专项 **26 通过**。工作区严格 Clippy、格式和源码行数检查通过。

真实 PTY 使用隔离的本机模拟服务，覆盖两个不同协议：

| 路径 | 验证结果 |
| --- | --- |
| 键盘从 alpha 切到 beta | 获取 beta 模型，确认后同步配置 |
| beta 前台 `/local` | 请求 Anthropic `/beta/v1/messages`，使用 beta 的 API Key |
| beta Runtime 与其子 Agent | 根任务和 reviewer 子 Agent 均请求 beta |
| beta 前台本地子 Agent | reviewer 子 Agent 同样请求 beta |
| 鼠标从 beta 切回 alpha | 获取 alpha 模型并确认，本地执行请求 OpenAI `/alpha/v1/chat/completions` |
| 身份与持久化 | 所有请求认证头正确，最终 Core 与 Runtime 的 Provider / 模型一致 |

证据见 [模型菜单终端画面](model-provider-menu.txt) 和 [请求与持久化验收结果](provider-switch.json)。这些证据来自本机协议模拟，不代表真实 Magpie 或上游模型性能验证；用户正在运行的 rc4 会话未参与测试。

在 macOS / Linux 上重跑：

```sh
cargo build -p willdeep --offline
ruby scripts/model_provider_tui_verify.rb "$PWD/target/debug/willdeep"
```

脚本创建独立临时目录、Provider 服务和 Runtime，使用测试占位凭据，结束后停止自己创建的 Runtime。

## 发布前完整检查

- CLI：768 通过、1 忽略；Core：783 通过、6 忽略。
- Runtime 集成：33 通过，包含有活动任务时平滑交接，以及静默后台任务的插话唤醒；版本一致性：1 通过。
- 工作区严格 Clippy、格式、源码行数、Web `yarn build` / `yarn lint`：通过。
- 原任务的备份、审批处理和安装后读回保存在用户本机私有验收目录，不随源码发布；不将远端评测已结束或 SSH 暂不可达等上下文误报为本次发布的模型验证结论。
