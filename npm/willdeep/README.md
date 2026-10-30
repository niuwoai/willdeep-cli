# @niuwoai/willdeep

自托管的 AI Coding Agent。模型可以不出国，每一行改动能追溯到是哪个 Agent、哪次工具调用改的，小模型交上来的活由退出码裁决而不是自我声明，检查点回退把对话和文件一起退回去。一个二进制，终端 / 浏览器 / 手机三种界面，关掉窗口任务照跑。

> **WillDeep** is a self-hosted AI coding agent. Every changed line is attributed to the agent and tool call that wrote it, sub-agent work is judged by exit codes, and sessions and goals outlive your terminal. Works with any OpenAI-compatible or Anthropic-style endpoint, including models that never leave your datacenter.

## 安装

需要 Node.js 18 或更新版本。安装器下载与本包版本对应的 [GitHub Release](https://github.com/niuwoai/willdeep-cli/releases)，校验 SHA256 后把单个 `willdeep` 放进 npm 的 bin 目录：

```bash
npm install -g @niuwoai/willdeep@rc
willdeep --version
```

| 平台 | 发行包 |
|---|---|
| macOS（Universal，Apple Silicon 与 Intel 通用） | `willdeep-macos-universal.tar.gz` |
| Linux amd64 / arm64 | `willdeep-linux-amd64.tar.gz` · `willdeep-linux-arm64.tar.gz` |
| Windows x64 | `willdeep-windows-x64.zip` |

## 开始用

没有配置文件也能跑，环境里有认得出的 Key 就直接开工：

```bash
export SOMEIM_API_KEY=...        # 或 ANTHROPIC_API_KEY
willdeep --onboarding            # 打开 WebApp 配置引导，也可选择终端手动配置

willdeep --workspace .                          # 终端界面（TUI）
willdeep --web --workspace .                    # 浏览器界面，默认 127.0.0.1:9847
willdeep web                                     # 复用或启动 Runtime Web App，并打开浏览器
willdeep qr                                      # 显示手机配对二维码
willdeep run --output json "总结当前风险"        # 无头自动化：稳定退出码，可进 CI
```

`willdeep doctor` 不联系任何 Provider 就能检查配置、围栏和 Runtime 状态。

更多内容见 [WillDeep 仓库](https://github.com/niuwoai/willdeep-cli)：[README](https://github.com/niuwoai/willdeep-cli#readme)、[docs/INSTALL.md](https://github.com/niuwoai/willdeep-cli/blob/develop/docs/INSTALL.md)、[docs/CONFIGURATION.md](https://github.com/niuwoai/willdeep-cli/blob/develop/docs/CONFIGURATION.md)。
