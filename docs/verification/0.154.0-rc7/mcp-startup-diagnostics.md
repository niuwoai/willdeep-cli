# MCP 启动失败诊断

## 历史事件

2026-11-20 23:31:32（UTC+8），本机 Runtime 的 `events.ndjson` 在 sequence 1020509、1020510 分别记录 `task.failed` 与 `turn.failed`，原因均为：

```text
initialize MCP servers: Resource temporarily unavailable (os error 35)
```

现存事件未记录服务名和失败阶段，不能根据错误码推断具体服务或系统资源。`task_manager` 保存了完整错误链；信息丢失发生在 MCP 层直接传播裸 I/O 错误时。

## 改动

- stdio 子进程创建与管道准备失败记录配置服务名、传输方式和实际失败阶段。
- HTTP 连接准备、`initialize` 握手、`notifications/initialized` 失败附带对应上下文。
- 原始错误通过 `source` 保留；直接显示错误时也包含底层原因。
- 服务名使用转义形式，避免换行和终端控制字符混入诊断。
- 不增加启动参数、环境变量或 URL 输出；保留 reqwest 的 `without_url()` 处理。
- 错误包装后仍能识别 HTTP 授权要求，保持配置状态报告语义。
- stdio 启动失败仍中止初始化；HTTP 失败仍告警并跳过；工具列表发现失败的原有容错不变。

示例（服务名为假设值，不代表历史事件已定位到该服务）：

```text
MCP server "example" (stdio) failed during process spawn: Resource temporarily unavailable (os error 35)
```

## 验证

新增五项 Rust 回归测试：真实不存在的命令、OS 错误与阶段保留、服务名控制字符转义、包装后的 HTTP 授权状态、真实 stdio 握手拒绝。OS error 35 使用构造的 I/O 错误验证，不耗尽真实系统资源。

首次构建与编辑并发，编译器在新测试模块文件写入之前读取了模块声明，报 `E0583: file not found for module startup_tests`。文件齐备后重新运行。

第二次 MCP 测试为 26 项通过、2 项失败：两个新增测试错误地将 `Error::source()` 向下转换为 `McpError`，而 `thiserror` 返回的动态类型为 `Box<McpError>`。已修正断言并重新运行；该次输出的错误文本、握手失败及授权状态测试均已通过。

最终 MCP 测试 **28 项通过、0 项失败**（其中新增 5 项），范围为 `willdeep-core mcp:: --lib`；其余 1351 项核心单元测试未执行。严格 Clippy（`willdeep-core --lib --tests -- -D warnings`）、版本一致性及格式检查通过。尚未提交、安装或发布；没有在用户实际 MCP 服务上主动重试，也未执行完整 CLI 构建或真实 TUI 验收。
