# 0.147.0-rc5 Web 修复版本协调

日期：2026-11-07。

## 版本归属

摘要复用分支 `refs/heads/claude/serene-heisenberg-0dc01e` 已提交 `82fbe778ffaae5948adfb6bf872038bf99836f29`，实际采用 `0.147.0-rc4`。通过读取 Git 提交对象核实，运行代码主要涉及 Core `agent.rs` 和 `agent/context.rs`，Web 仅有 package 版本元数据更新。

Web 修复分支 `refs/heads/fix/web-error-child-proposal-20261107` 从共同基线 `ec565d68657eeaf97e0ffc2467c7bd6b49a65e3d` 独立开发，尚未提交。本轮将 Web 修复元数据顺延为 `0.147.0-rc5`，覆盖 Cargo workspace、内部 crate 版本钉、Cargo.lock、Web/npm 包及安装清单、CHANGELOG、产品概览、架构与使用文档。隔离 API 验收脚本预期版本同步为 rc5。

两个分支尚未整合；rc5 的版本号不表示已包含摘要复用修复。尚未提交、推送、发布、安装本 Web 分支，也未创建或移动 tag。

## 证据边界

Web 运行代码没有因本次版本协调发生变更。731 个 Rust 源文件及 Web src 文件的集合与内容 SHA256，在协调前后均为：

`5a11da514bb1691d1d4c03b181f7600522fbfc0654e904ec6932d31996fe8dfb`

此前 423 项 Web 测试、25 项 Rust 定向回归、10 项隔离 Runtime API 验收、构建、Clippy 和浏览器验收，执行对象为本 Web 分支暂用 **rc4** 的构建；完整原始说明见 [rc4 Web 验收](../0.147.0-rc4/README.md)。原 JSON、截图和 API 回执不改写成 rc5。

本次 rc5 仅进行版本一致性、Cargo 锁定元数据解析、Ruby 验收脚本语法、差异空白和运行代码摘要核对。没有重新构建 rc5 二进制或重跑浏览器，因此不能把现有 rc4 预览实例当作 rc5 运行验证。现存实例仍是 rc4；停止操作此前被自动审批拒绝，本次未重试或换工具停止服务。

后续若整合摘要复用分支，应以最终组合源码重新构建和验证，并协调共享版本元数据及 CHANGELOG，不能直接将两份独立测试相加视为整合通过。
