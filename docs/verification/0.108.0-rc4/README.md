# Linux 后台等待测试权限隔离

日期：2026-10-20。版本：0.108.0-rc4。

rc3 的 macOS、Windows 测试及四个平台产物构建通过，Linux 两种架构均在新增长 sleep 用例等待 Partial 检查点时超时，因此未创建公开 Release。

以 CI 构建的 Linux arm64 二进制在隔离 Ubuntu 24.04 容器复现：默认工作区实际权限是 workspace-write，审批追踪为“no OS write fence”，任务状态为 waiting_approval；仅修改配置文件中的 approval 不会覆盖 Runtime 已登记的工作区策略。显式登记 full_access 后，同一命令立即返回 background-delay 通知，检查点为 partial，任务继续运行。

修复只调整新增 Runtime 集成测试：在独立临时目录中显式注册完全访问工作区，测试后台等待与插话递送，不再依赖 runner 是否支持 OS 围栏。产品审批与围栏规则不改；危险命令仍由现有 Core 回归验证不会绕过审批。

真实 Linux TUI 联调通过：输入区回车插话在 sleep 完成前被读取，命令执行一次，插话保存一次，完成结果交回模型，并显示可继续输入提示。使用 rc3 Linux arm64 CI 产物（rc4 产品代码不变），证据见 [linux-tui-report.json](linux-tui-report.json)。

修复后的后台插话集成测试 3 项通过（显式后台作业、监视器、自动后台长 sleep）；严格工作区 Clippy、Web build、格式、版本同步和源码大小检查通过。重发流水线负责完整跨平台复验。

完整功能与优化构建实测证据见前序 0.107.0-rc1、0.108.0-rc1、rc2、rc3 验收目录。本版本通过递增 rc 发布，不移动已推送标签。
