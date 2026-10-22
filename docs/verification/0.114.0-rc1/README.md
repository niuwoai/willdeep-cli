# 0.114.0-rc1 验收

日期：2026-10-22。真实 macOS CLI、独立临时工作区与 WILLDEEP_HOME、回环协议服务、PTY 和 xterm 终端重建。所有推理输入均为合成数据。

## 模型菜单

- `scripts/model_cascade_tui_verify.rb`：18 步通过。宽窗口 Provider / 模型两栏、独立搜索、当前模型定位、缓存刷新、键盘确认、鼠标预览与确认、52×16 窄窗口返回、重新放大、取消保留原 Provider / 模型。
- 浏览不写配置、不改变活动模型；确认后核验 Core Session、Runtime Session 和实际 HTTP 请求的 Provider、模型及认证。
- `scripts/model_provider_tui_verify.rb`：OpenAI → Anthropic → 鼠标切回，前台、Runtime 与两次子 Agent 请求均使用正确认证和模型。
- 默认从当前模型开始；Tab 切栏，↑↓ 导航，Enter 确认；窄窗口 Esc 返回 Provider，宽窗口 Esc 取消。保留 Ctrl+P，F5 刷新。

## 本地服务发现

- `scripts/local_provider_discovery_tui_verify.rb`：11 步通过。启动发现 Magpie，启动后 Ollama 上线，F5 自动加入；两者均可在菜单确认后完成真实 Runtime 回答，重开无重复 Provider。
- 原配置前缀（含注释、默认 Provider）保留，已有地址复用，名称冲突另起本地名称。探测不携带云端密钥，实际调用只使用各本地 Provider 的占位凭据。
- 6 项专项单测覆盖双服务、无凭据、错误服务、回环约束、首次创建、默认值与注释保留、地址去重、名称冲突、0600 文件权限。
- 本机实际服务只读探测：Ollama `/api/tags` 与 Magpie `/api/hello` 均 HTTP 200，身份结构正确；没有向实际服务提交推理内容。
- 默认 Ollama 11434 / Magpie 3425；支持本地 OLLAMA_HOST、MAGPIE_ADDR、Magpie XDG 配置端口。单请求 700ms 超时，禁止代理和重定向。打开菜单和 F5 的发现均在后台执行。
- `WILLDEEP_LOCAL_DISCOVERY=0` 关闭自动发现；不会自动启动服务或下载模型。

## 流式显示

修复原理及修复前后对照见 [0.113.0-rc3 流式验收](../0.113.0-rc3/streaming-latency.md)。主要瓶颈是 TUI 在每个流式片段上同步读写完整会话，而非 Responses / Chat Completions 的接口选择。

合入选择器后的一轮并行验收出现 P95 4668ms，160 片均显示；保留此异常，不作为通过结果。随后独立复测同一优化版：中位 14.71ms、P95 114.61ms，160/160 显示；同机参考修复版中位 10.99ms、P95 56.84ms。该异常尚不能仅凭单次结果归因为主机负载，最终优化版另作串行验收。

脚本现在即使延迟门槛失败也会保存 JSON / Markdown 报告。验收门槛要求全部片段到屏、P95 ≤200ms，并核验完整回答落盘和退出游标刷新；不以服务端已发完代替屏幕已显示。

## 其他检查

- CLI 单测：838 通过、0 失败、3 项原有忽略项。
- Web 0.114.0-rc1 构建通过；文件尺寸边界通过。
- 从真实可执行文件首次启动提取 Favorites，清单与 MCP `serverInfo.version` 均为 2.3.0；`server/favorites.rb`、`server/rich_content.rb`、`ui/index.html` 与打包输入逐字节一致。
- 官网下载入口不包含当前 CLI 的固定版本；历史基准记录中的旧产物地址是历史证据，不改写。
