# 本机安装与 Runtime 交接

日期：2026-11-23。版本：`0.154.0-rc8`。安装源码提交：`d06b4fb7754edafc7d6756410fde99bd4cb4e80a`。此次为本机候选安装，尚未公开发行。

## 构建与入口

已先执行 `yarn --cwd web build`，再从干净工作树执行：

```bash
cargo install --path crates/willdeep-cli --locked --offline --target-dir .build/cargo-target --force
```

优化构建及安装成功。`$HOME/.cargo/bin/willdeep` 与 `/opt/homebrew/bin/willdeep` 均报告 `willdeep 0.154.0-rc8`；后者是指向前者的符号链接，因此两个入口使用同一产物，无需更新公开 Homebrew tap。

安装产物 SHA256：`3b415329d49bd08efb37b5b73fe653bb489c19d86c27b8440061cf124d2786a9`。

Cargo 对锁文件中的 `chacha20 0.10.1`、`clipboard-win 5.4.1` 给出了 registry yanked 警告；本轮保留锁文件中的依赖版本，离线优化构建成功。

## Runtime 与真实交互

升级前只统计任务状态，未发现运行中、排队或等待人工的任务。执行 `willdeep daemon upgrade --timeout 60` 后，Runtime 从 `0.154.0-rc7` 平滑交接到 `0.154.0-rc8`。`daemon status` 显示 running，`runtime.status` 返回 `status: ok`，客户端、服务版本均为 rc8。本次没有数据库结构变更，未执行数据库迁移或回滚。

使用实际安装产物执行：

```bash
ruby scripts/verify_tui_input_ownership.rb "$HOME/.cargo/bin/willdeep" .build/input-ownership-installed-run1
```

full / compact 两种布局的 14 项交互检查和 1 项二进制 SHA256 检查全部通过，10 项夹具清理检查全部通过。验收前后安装产物校验值相同。覆盖异步提问恢复捕获、单选和多选真实工具回执、框外点击、正文滚动、缩放、原生选择冻结与粘贴恢复。原始 JSON、Markdown、ANSI 和终端帧保留在本工作树 `.build/input-ownership-installed-run1/`。

本轮 TUI 原生测试为 707 通过、2 忽略；两项既有忽略项分别是手动确定性流式渲染性能测量、手动 paced-stream 显示延迟测量，未执行。其他验证及范围见[交互验收记录](tui-input-ownership.md)。

## 备份与回滚

旧版 `0.154.0-rc7` 已备份到：

`$HOME/.local/share/willdeep/binary-backups/0.154.0-rc7-20261123-tui-input-ownership/willdeep`

旧产物与备份 SHA256 一致：`b997a009dc40c99095822bacaf1d30af1d0c7f2dd7737d0bce870384bb5af764`。此次没有清理历史备份。

如需回滚，先完成正在等待人工的任务，再恢复旧产物并平滑交接 Runtime：

```bash
cp "$HOME/.local/share/willdeep/binary-backups/0.154.0-rc7-20261123-tui-input-ownership/willdeep" "$HOME/.cargo/bin/willdeep"
```

```bash
"$HOME/.cargo/bin/willdeep" daemon upgrade --timeout 60
```

```bash
"$HOME/.cargo/bin/willdeep" --version
```

```bash
"$HOME/.cargo/bin/willdeep" daemon status
```

回滚命令依照 Runtime 交接实现核对，本次未实际执行回滚。若交接因等待人工任务而拒绝，先完成该任务后重试；若超时则按命令提示处理，不强行停止任务。

替换可执行文件不会热更新已经运行的 TUI；现有 TUI 需要正常退出后重新打开，才能加载新交互。验收不覆盖宿主终端保存备用屏历史的策略，未覆盖其他操作系统或真实模型。
