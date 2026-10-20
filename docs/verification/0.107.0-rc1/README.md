# 项目会话启动选择验收

日期：2026-10-20。开发版本：0.107.0-rc1。

## 行为

- 无参数交互启动：按工作区真实路径选择最近的非空 Rust 会话，排除其他工作区、空白记录与 Xedit 桥接记录；没有候选时直接新建。
- 有候选时显示标题、路径与最后使用时间，默认继续，5 秒无操作自动进入。任意按键或鼠标操作暂停倒计时；方向键/Tab 与 Enter、Y/N 和鼠标点击均可操作，选项保持单行，中间空白不响应点击。
- `willdeep config startup ask|latest|new` 保存偏好，`--session-startup` 单次覆盖，`--new-session` 本次新建；显式恢复、命令行提问、管道输入、JSON、Web 与 Runtime 不使用启动询问。
- 选择继续后重新读取最新会话，再进入已有 TUI/Runtime 跟随流程；选择本身不写会话、不提交消息、不重跑工具。

## 验证

| 检查 | 结果 |
| --- | --- |
| `cargo test -p willdeep --offline --quiet` | CLI 单测 773 通过、1 忽略；辅助集成 18 通过、1 忽略；Runtime 集成 33 通过；版本一致性 1 通过 |
| `cargo clippy --workspace --all-targets --offline -- -D warnings` | 通过 |
| `cargo fmt --all --check`、`git diff --check`、源码行数检查 | 通过 |
| `yarn build`、`yarn lint` | 通过；Vite 保留既有大包体积提示 |
| `cargo build -p willdeep --offline` | 通过，开发二进制版本为 0.107.0-rc1 |
| `ruby scripts/session_startup_tui_verify.rb <开发二进制绝对路径>` | 11 项真实 PTY 场景通过 |

PTY 覆盖：无操作超时继续、N 新建、方向键选择新建后等待超过 5 秒再确认、鼠标继续、鼠标新建、单次新建、显式恢复、首次使用工作区、保存总是继续、保存总是新建、单次覆盖偏好。每项进入主界面后都输入 `#1983#` 打开本地确认示例，验证启动窗口与主界面的输入读取可以正常衔接。

测试使用独立临时 home、工作区、历史夹具与回环 Provider，不接触用户真实会话或 Runtime。三份原会话的消息逐条不变；未生成持久化空会话。模型推理请求为 0，既有模型能力元数据 GET 为 22。原始 ANSI 与 JSON/Markdown 报告由脚本保存到其输出的临时目录。

最初沙箱内的完整测试因禁止回环监听失败，授权环境重跑后上述测试全部通过。此记录仅证明本地开发验证，不代表已提交、集成、发布或安装。
