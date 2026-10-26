# 首次使用：在 WebApp 配置 Provider

首次在交互终端运行 `willdeep`，且尚无可用的 Provider 配置或环境变量时，
CLI 自动在本机回环地址启动 WebApp 并显示内置快速设置；左下角的「配置」入口也可以随时打开完整配置工作台。
用户添加 Provider 和模型并保存为默认配置后，终端自动继续。
也可用 `willdeep --onboarding` 再次打开引导；`willdeep --web` 在全新环境同样输出配置链接。
无桌面浏览器或浏览器启动失败时，手动访问终端打印的 URL；SSH 用户可转发该端口。
非交互式运行不会打开浏览器或弹交互提示。

1. 在快速设置中新增 Provider，填写 API Base、API Key
   （或 API Key 环境变量名）、模型与协议，并选择默认 Provider。
2. 左下角「配置」入口包含完整配置工作台：schema 表单与快照、变更预览/应用、校验、备份/恢复、模板生成、Provider 模型列表，以及 Provider、MCP 服务和子代理集合管理。
3. 点击保存。宿主检查配置能否解析出默认 Provider 的地址、
   凭据和模型；此检查不调用模型，不验证账户余额或服务端认证是否成功。
4. 保存后 CLI 自动重新读取配置，继续进入终端 TUI，无需退出或重新启动。
   配置 WebApp 在本次 CLI 进程存活期间继续可用，退出 CLI 后关闭；等待期间可按 Ctrl+C 取消。
   显式运行 `willdeep --web` 时仍使用前台 Web 模式。

完整配置工作台复用随 CLI 内置的 `willdeep-config` 配置引擎，但不要求用户在插件中心启用插件。
校验、备份/恢复和模型列表功能需要 Ruby；macOS 使用系统 Ruby，Linux 可安装 Ruby。
快速 Provider 设置由 Rust Web API 提供，不依赖 Ruby；缺少 Ruby 时仍可先配置一个默认模型。
无法使用浏览器时运行 `willdeep --onboarding`，选择 **3) 终端手动配置**。
原 some.im 浏览器登录保留在选项 2。

插件操作的是宿主的实际配置文件（包含 `--config` 和 `WILLDEEP_HOME`），备份写入
当前 WillDeep home 的 `plugin-data/willdeep-config/backups`。输入和输出均不在引导里回显密钥。
已通过环境变量配置的 CLI 用户直接启动，不重复进入引导；引导中的「稍后配置」只隐藏当前页面提示。
