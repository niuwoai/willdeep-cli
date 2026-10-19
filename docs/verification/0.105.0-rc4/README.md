# 后台等待时自动发送排队消息验收

日期：2026-10-20（Asia/Shanghai）。版本：0.105.0-rc4。以下记录发布前的源码与隔离运行验收；公开产物和用户本机安装另行读回确认。

## 行为

Runtime 在主模型循环已返回、仅剩后台工作时发送 `background_wait` 边界事件。TUI 收到该事件后，自动使用现有 `turn.steer` 通道发送队首普通文本，成功后才移除该消息。下一条消息等待新的边界；模型生成、工具执行、审批或问答期间不据此自动发送。

队列或附件面板打开时暂停自动发送。命令和带附件的消息仍按原有顺序等待完整轮次结束；不跳过队首，不拆分附件。发送失败保留消息并打开队列显示错误。后台任务和输入框草稿不因发送而结束或重置。

## 验证

- `cargo test -p willdeep --bin willdeep tui:: --offline`：244 项通过。
- `cargo test -p willdeep --test headless_runtime background_steering --offline`：静默 monitor 和 detached job 两项通过；额外断言 Runtime 发布后台等待边界，且模型收到插话时后台尚未退出、会话中消息恰好一份。此测试需要本机端口权限；首次沙箱运行因拒绝监听测试端口失败，允许本机监听后通过。
- `cargo clippy --workspace --all-targets --offline -- -D warnings`：通过。
- `cargo fmt --all --check`、`git diff --check`、源码行数检查：通过。
- Web `yarn build`：通过。初次构建因本地依赖目录缺少类型文件失败，按现有锁文件恢复依赖后通过，未修改依赖版本。
- 发布前 `cargo test --workspace --offline`：完整通过。CLI 756 项通过、1 项忽略；core 782 项通过、6 项忽略；Runtime 33 项；detached jobs 18 项通过、1 项忽略；版本一致性、Provider 合同、Runtime SDK 与文档测试均通过。
- Web `yarn lint`：通过。

## 真实终端与 Runtime 链路

使用新编译的 debug 二进制、独立临时 home/workspace 和本机确定性模拟 Provider，未使用用户凭据，也未替换现有 Runtime。

1. 在真实 PTY TUI 启动静默 monitor：等待 `release-background` 文件，90 秒上限。
2. 模型先回复后台仍在运行，Runtime 发布等待边界。
3. 输入 `AUTO-QUEUE-BOUNDARY-20261020`，只按普通回车；没有打开队列、按 S 或通过 API 人工插话。
4. TUI 自动发送；事件序号 22 记录 `turn.steered`，序号 23 清除等待标记，同秒进入新轮次；终端显示模型已读取消息，队列和待读取状态清空。
5. 读取后核实原任务仍为 `running`、释放文件不存在，用户消息恰好一份。
6. 创建释放文件后，monitor 以 `reason: exited`、`exit_code: 0` 正常结束，持续 30 秒，未触发超时；任务为 `completed`，TUI 显示 `Ready / Your turn`。

测试 task：`502656c8-5a80-444e-99b9-ec23f2e8e9ea`。
测试 session：`85ea2841-7707-4083-ac83-8aa2053a8bef`。
本机原始证据：`/private/tmp/willdeep-autoqueue-direct/`。测试 TUI 和隔离 Runtime 已停止。

终端验收夹具前几次受首次引导、配置权限和 PTY 输入方式影响未进入任务，未据此判断产品回归。最终采用工具提供的真实 PTY、明确测试配置、分别发送文本和回车完成上述链路；未调用上游付费模型。该验收证明客户端与 Runtime 自动投递路径，不代表上游模型的响应速度。
