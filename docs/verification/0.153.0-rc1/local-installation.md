# 0.153.0-rc1 本机编译安装与 Runtime 交接

本机操作日期：2026-11-18。源码来自远程 develop，提交 `8372c94203ea5514d289a857e3f953d8ab360c83`。未修改产品代码，未打 tag、修改远程 Homebrew tap 或公开发布。

## CI 核查

核查快照时间：2026-11-18 00:15:51（北京时间，UTC 为 2026-11-17 16:15:51）。

- [PR155 工作流](https://github.com/niuwoai/willdeep-cli/actions/runs/44093578622) 和 [PR156 工作流](https://github.com/niuwoai/willdeep-cli/actions/runs/44089881982) 全部通过。
- [最新 develop 工作流](https://github.com/niuwoai/willdeep-cli/actions/runs/44097334755) 的 macOS 测试、Linux x86_64、Linux aarch64、WSL 冒烟共四项通过，无失败。
- Ubuntu 测试排队：匹配的 `willdeep-q4-pub` 自托管 runner 离线、busy=false；job 未分配 runner，尚未执行任何步骤。两个 PR 的 push 工作流也各剩该排队项，因此未把整套 CI 标为全绿。

## 构建与验证

独立工作树 `local-install-develop-20261118` 中先执行 yarn 锁文件安装和 Web 构建，再以 `--locked --release -p willdeep` 构建 Rust；复用本会话 target 缓存。将最终二进制冻结后，在独立 HOME、私有 Runtime 和合成工作区验收，最后安装完全相同的字节。

- 应用版本：`0.153.0-rc1`。
- SHA256：`c3ebd07977960911cfb291a268e01900929e3b6d09556a827b2fb75892a540f9`。
- `/health` 版本正确；内嵌首页及其 JS/CSS 与本轮新构建的 web/dist 逐字节/摘要比较通过。
- 真实 API/PTY 验收 8 项通过，涵盖群成员过滤、裸引用、会话隔离及 /groups。
- 浏览器确认第七位成员 zebra 可检索，选中后插入裸 @zebra 并关闭候选面板；当前网络无失败请求。验收中误触会话重命名触发预览容器不支持 window.prompt 的控制台错误，该交互未计入通过项。
- 两处入口与冻结产物 SHA256 一致；两处 doctor 均验证 Web 资源可用和 Runtime 版本一致。
- Runtime PID：`30640`；实际运行 `/Users/rocky/.cargo/bin/willdeep`，摘要与安装产物一致。
- doctor 总体状态为 warning：LSP 依赖 2/2 可用、配置 1/2 有效，另一个配置被校验拒绝；ast-grep 未在可信绝对 PATH 中找到。本次没有修改这些配置或安装额外依赖。
- 首次安装前检查因停止状态输出匹配过严而退出，安装未执行；按真实 CLI 输出校正后确认私有 Runtime 已停止，重试安装成功。

## 安装入口

| 入口 | 实际文件 | 版本 |
|---|---|---|
| `/Users/rocky/.cargo/bin/willdeep` | `/Users/rocky/.cargo/bin/willdeep` | `willdeep 0.153.0-rc1` |
| `/opt/homebrew/bin/willdeep` | `/opt/homebrew/Cellar/willdeep-cli/0.149.0-rc12/bin/willdeep` | `willdeep 0.153.0-rc1` |

保留 `/opt/homebrew/bin/willdeep` 的符号链接，只覆盖本机实际文件；Homebrew formula 的目录名不代表当前试装文件版本，后续 brew 管理操作可能替换它。

通过 `willdeep daemon upgrade --timeout 300` 正常交接，未使用 --force 或强制停止；任务在交接前无运行中或等待人工响应项。

## 备份与回滚

备份目录：`/Users/rocky/.local/share/willdeep/backups/20261118-rc4-before-0.153.0-rc1`。两个旧文件均为 0.152.0-rc4，SHA256 为 `a776238e14831a97592c56704e3d56945ddb6500ea48ad0fde1eb99ac7511e44`；安装前逐字节比较通过，既有备份均保留。

以下为需要回滚时的命令，本次未执行。先处理活动任务和人工请求，再恢复两个入口并正常交接 Runtime。

```bash
test ! -e "$HOME/.cargo/bin/willdeep.restore-rc4" && install -m 755 "$HOME/.local/share/willdeep/backups/20261118-rc4-before-0.153.0-rc1/cargo-willdeep" "$HOME/.cargo/bin/willdeep.restore-rc4" && mv "$HOME/.cargo/bin/willdeep.restore-rc4" "$HOME/.cargo/bin/willdeep"
```

```bash
test ! -e /opt/homebrew/Cellar/willdeep-cli/0.149.0-rc12/bin/willdeep.restore-rc4 && install -m 755 "$HOME/.local/share/willdeep/backups/20261118-rc4-before-0.153.0-rc1/homebrew-willdeep" /opt/homebrew/Cellar/willdeep-cli/0.149.0-rc12/bin/willdeep.restore-rc4 && mv /opt/homebrew/Cellar/willdeep-cli/0.149.0-rc12/bin/willdeep.restore-rc4 /opt/homebrew/Cellar/willdeep-cli/0.149.0-rc12/bin/willdeep
```

```bash
willdeep daemon upgrade --timeout 300
```

已打开的旧 TUI 进程不会随文件替换而变成新版；保留草稿、正常退出后重新运行 willdeep 即可使用新版本。
