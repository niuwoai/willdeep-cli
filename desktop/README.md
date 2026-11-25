# WillDeep Desktop

桌面端和浏览器共用仓库的 React 工作台。Mac 内测包随附同版 Rust Runtime，连接已有兼容 Runtime 或启动本机实例。运行桌面时，本机 Runtime 会保持连接。桌面退出后，已提交的任务仍由 Runtime 管理。

## 构建 Mac 安装包

在 macOS 上准备 Xcode 命令行工具、Rust、Node.js 和 Yarn。默认构建 Apple Silicon 与 Intel 通用包，因此需要两个 Rust 编译目标。

```sh
rustup target add aarch64-apple-darwin x86_64-apple-darwin
yarn --cwd web install --frozen-lockfile
yarn --cwd desktop install --frozen-lockfile
yarn --cwd desktop bundle
```

安装包生成在仓库的 `.build/desktop/`，命令会打印文件路径与 SHA-256。只构建本机 Apple Silicon 调试包可用 `yarn --cwd desktop bundle --target aarch64-apple-darwin`。内测包使用本地临时签名。公开分发使用发行签名与公证流程。

Windows 使用 WebView2 与本地命名管道，Linux 使用 WebKitGTK 与 Unix Socket。构建入口可在目标系统运行，分别采用 NSIS 和 DEB。平台依赖与工具链见 [Tauri 构建要求](https://v2.tauri.app/start/prerequisites/)。

## 使用与验证

从「文件」菜单或项目栏打开目录，新会话沿用工作区的模型和权限配置。顶部文件图标打开右侧改动面板，选择文件查看 Diff。关窗后可从桌面或 CLI 进入同一个 Runtime 会话。桌面侧栏显示应用与当前连接的 Runtime 版本。

运行 `ruby desktop/verify.rb <应用内的 willdeep 可执行文件>` 可用隔离 Runtime 和本地模拟模型复测鉴权、重连、审批和 Diff。报告写入 `.build/desktop/verification/`。

架构、功能范围和跨平台验收路径见[桌面版规划](../docs/product/desktop.md)。
