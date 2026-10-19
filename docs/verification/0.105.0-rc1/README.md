# Magpie 配置统一验收

- 开发版本：`0.105.0-rc1`；分支：`codex/magpie-provider-config`。
- 验收基线：`07451a0`；本次仅集成 `develop`，未安装或发布。
- 真实 Magpie：官方源码 `224ea7d5bee5759e1e2bfeac8aca4a0c3d9503a5`，临时目录以 `nogui` 构建，未全局安装。
- 上游：本地确定性模拟服务；未使用真实账号和凭据，付费请求为 0。

## 结果

| 验证 | 结果 |
| --- | --- |
| CLI 单元测试 | 754 通过，1 项原有跳过，0 失败；包含 11 项新增配置规则测试 |
| Runtime 进程集成 | 31 通过，0 失败；新增用例覆盖前台和 Runtime 中显式子 Agent 的地址与模型 |
| 严格 Clippy / 格式 / 文件行数边界 | 通过 |
| Web 构建 | 通过 |
| 真实 Magpie 模型目录 | 通过 |
| OpenAI 标准环境变量、SSE、只读工具往返 | 通过，2 次模拟上游请求 |
| Anthropic 标准环境变量、Messages、SSE | 通过，1 次模拟上游请求 |
| 本地 Profile → 显式子 Agent → 父 Agent | 通过，3 次模拟上游请求 |
| Runtime Profile → 显式子 Agent → 父 Agent | 通过，3 次模拟上游请求，并检查 Runtime 任务记录 |

详细机器记录见 [JSON](magpie-integration.json)，脚本直接生成的摘要见 [Markdown](magpie-integration.md)。

## 边界与复跑

这是“真实 Magpie 网关 + 真实 WillDeep 二进制 + 本地模拟模型上游”的协议和执行链路验收，不代表所有真实供应商、订阅及模型组合均已验证。首次夹具使用 macOS 默认长临时路径时触发 Unix socket 路径长度限制，改用 `/private/tmp` 短路径后复跑通过。

前台和 Runtime 的集成测试使用隔离配置目录；真实 Magpie 验证另隔离子进程 HOME，关闭账号发现及遥测，退出时关闭测试 Runtime 和网关。用户正在使用的 Runtime 未被本次任务升级或重启。

复跑方法及配置优先级见 [Magpie 接入指南](../../MAGPIE.md)。
