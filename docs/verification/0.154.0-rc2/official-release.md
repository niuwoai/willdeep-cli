# 0.154.0-rc2 正式发行、安装渠道与官网验收

日期：2026-11-19。CLI 0.154.0-rc2 已公开发行，npm 的 rc/latest 已同步；本机 Cargo、Homebrew 和 Runtime 均为 rc2。官网 1.12.0-rc14 已部署并通过线上内容核对。本记录补充发行后的实际结果，不移动已发布 tag，也不变更运行时代码或应用版本。

## 发行身份

- [规则扫描修复 PR 162](https://github.com/niuwoai/willdeep-cli/pull/162) 与[发行准备 PR 163](https://github.com/niuwoai/willdeep-cli/pull/163)均已合入默认分支 develop。
- CLI annotated tag `v0.154.0-rc2` 指向 `0fb3a57bf1811c0bea767767bad0bb0befd31e5b`，发行时 develop/main 与该提交一致；后续纯文档提交不改变此 tag。
- [Release 工作流 44448133912](https://github.com/niuwoai/willdeep-cli/actions/runs/44448133912)的 10 个任务全部成功：两项平台测试、两项 Linux 架构测试、WSL ABI 检查、三项归档构建、GitHub Release 发布和 npm 发布。
- [GitHub Release](https://github.com/niuwoai/willdeep-cli/releases/tag/v0.154.0-rc2)对象 ID 为 `450858598`，公开时间 `2026-11-19T03:54:50Z`，非 draft。GitHub 实际分类为非 prerelease，但版本名仍含 rc，本记录仍称候选版。

## 官方归档与 npm

| 归档 | 实际下载 SHA-256 |
| --- | --- |
| `willdeep-linux-amd64.tar.gz` | `23ae891475dbfb810ba33c8dc9873fd0ab4033bee4d05c8a2dfd5edb06f9f361` |
| `willdeep-linux-arm64.tar.gz` | `d81509864d279b1daea996d6dfae3fa893b81852c52cdd3d710075e144865713` |
| `willdeep-macos-universal.tar.gz` | `904f8fa781c01d9a9d8e776d0177e219f899cefd9496bb06a609e7936ab68ea0` |

三归档均以不携带认证的 HTTPS 请求实际下载；gzip 格式和完整性、唯一的 willdeep 成员、普通文件类型、执行权限与架构通过检查，摘要与 GitHub asset digest 精确一致。macOS universal 含 x86_64/arm64 两切片，宿主执行报告 `willdeep 0.154.0-rc2`。Linux 二进制在本机只做静态检查；对应 Linux CI 成功，不声称在 macOS 原生运行过 Linux 二进制，也不声称分别实跑 macOS 两个切片。

npm 发布日志确认：`2026-11-19T03:55:07.5406957Z` 发包成功，`03:55:08.2136452Z` 同步 rc，`03:55:08.8353030Z` 同步 latest。强制在线查询确认 `rc=latest=0.154.0-rc2`；实际发布包的安装清单版本、仓库、tag、三资源名与摘要均正确。在独立前缀中实际执行无标签安装及 postinstall，安装后的命令输出 `willdeep 0.154.0-rc2`。

默认安装命令：

```bash
npm install -g @niuwoai/willdeep
```

显式选择候选通道仍可使用：

```bash
npm install -g @niuwoai/willdeep@rc
```

`latest` 表示默认安装通道，不会消除版本名中的 rc。发布脚本防止旧任务回退标签，两个工作流串行更新；失败和重试边界见[npm 通道记录](npm-dist-tags.md)。[官方产物机器可读结果](official-release.json)记录实际发行和安装检查。

## Homebrew 与本机 Runtime

[Homebrew PR 18](https://github.com/niuwoai/homebrew-tap/pull/18)两项 CI 通过，合并提交 `8667929c525d6f4d0e97a4fcb06c6260838dcc92`，时间 `2026-11-19T04:17:56Z`。配方固定上述 macOS universal URL 与真实摘要，同步 Tap README 和 CHANGELOG。

本机只快进目标 Tap，保留既有 `PENDING.md` 与 `.claude/` 修改，不使用共享 stash；升级时关闭自动更新和安装清理。实际升级、`brew test`、以已安装配方名执行的 `brew style --formula` 和 `brew audit --strict --online --formula` 均退出 0。

| 入口 | 实际状态 |
| --- | --- |
| `/Users/rocky/.cargo/bin/willdeep` | `willdeep 0.154.0-rc2`，源码构建安装 |
| `/opt/homebrew/bin/willdeep` | 链接至 `../Cellar/willdeep-cli/0.154.0-rc2/bin/willdeep`，执行报告 rc2 |
| Runtime | rc2，PID 61794，控制地址 `127.0.0.1:55954`，Homebrew 升级期间未重复重启 |
| Runtime Web | 发布前真实检查 `/health` 的 HTTP 状态、正文和版本头均正确；首页三资源摘要与本机构建一致 |

Homebrew 安装的 universal 二进制摘要为 `64f473f5bd1298f394e2117290d355e39d5e92515ce6fa45e20f9f5d29a5dd54`，包含 x86_64/arm64 两片。Cargo 安装文件摘要为 `72a55e6d15b9c227d81428c4f185a4ff3be7fa746057d40177bd7ae5e44cd05c`；源码构建文件与官方 universal 文件不同，不能要求二者摘要相同。

原 rc1 Cellar 保留，既有备份未清理。Homebrew 曾报告其他 Tap 的既有信任警告，本轮未改变其信任配置。早期对外部路径的 style 尝试不适用于配方检查，未计为通过；正式配方名检查已通过。详见[Homebrew 机器可读结果](homebrew-installation.json)及[发布前本机安装记录](local-installation.md)。

## 官网部署

[官网 PR 36](https://github.com/niuwoai/site.willdeep/pull/36)已于 `2026-11-19T04:23:48Z` 合入 main，提交 `d7f6e488ea4a03e6a902b726cdbecb7f51e82157`；annotated tag `v1.12.0-rc14` 指向该已合入提交。Railway 生产部署对应 GitHub deployment `7904044991`，于 `04:24:42Z` 报告 success。

官网保留自己的版本 1.12.0-rc14；首页 CLI 卡片、默认 npm 命令、中英文当前产品概览和更新记录、llms 索引同步 CLI rc2。当前区域无待发布文案，历史教程、评测和其他产品内容保留原范围。

在官网独立 worktree 中使用指定 Yarn 4.12.0，31 项测试全部通过，TypeScript 检查和 Vite 6.4.3 生产构建成功。保留既有 React act 提示和大体积 chunk 警告。最终本地浏览器验证了默认 npm 命令、Homebrew 切换、双语更新记录及固定 Release 链接，没有控制台错误。

`2026-11-19T04:26:01Z` 实际 HTTPS 核对：

- `/`、`/products/cli/`、`/products/cli/changelog` 均 HTTP 200，`X-App-Version: 1.12.0-rc14`。
- 中英文产品概览、更新记录和 `/llms.txt` 共五文件均 HTTP 200，版本头正确，SHA-256 与本地最终源码及 dist 精确一致。
- 入口 JS/CSS 均 HTTP 200；生产 JS 包含网站 rc14、CLI rc2 与新的默认 npm 安装命令。
- 本地浏览器直接跳转公网被自动审批以 `Browser Navigate Exfil` 拒绝，因此线上使用无 Cookie、无认证的只读 HTTPS 核对。未声称完成公网浏览器交互。

详细路由、文件摘要、入口资源和检查边界见[官网部署 JSON](website-deployment.json)。

## 备份与回滚

本轮没有数据库迁移，没有更新远端 q3。以下命令为出现问题时的恢复步骤，未实际执行。

本机 CLI 备份目录：`/Users/rocky/.local/share/willdeep/backups/20261119-rc1-before-0.154.0-rc2`。其中 Cargo/Homebrew 原二进制在备份时均与原文件逐字节比较一致，至少六份更早备份继续保留。

| 备份 | 原版本 | SHA-256 |
| --- | --- | --- |
| `cargo-willdeep` | 0.154.0-rc1 | `29299cb15eeea55d53ab20dd5a6b9b0fcf121b15773c9749e5af717a7a254a75` |
| `homebrew-willdeep` | 0.154.0-rc1 | `2228fb61781c264c009a28c9902957b6b8c3251ad8bc5d00b758634ec0366b29` |

先处理活动任务及人工请求，再恢复 Cargo 命令并正常排空交接 Runtime：

```bash
test ! -e "$HOME/.cargo/bin/willdeep.restore-rc1" && install -m 755 "$HOME/.local/share/willdeep/backups/20261119-rc1-before-0.154.0-rc2/cargo-willdeep" "$HOME/.cargo/bin/willdeep.restore-rc1" && mv "$HOME/.cargo/bin/willdeep.restore-rc1" "$HOME/.cargo/bin/willdeep"
```

```bash
"$HOME/.cargo/bin/willdeep" daemon upgrade --timeout 300
```

Homebrew 的旧安装 `/opt/homebrew/Cellar/willdeep-cli/0.154.0-rc1/bin/willdeep` 仍保留。以下仅在当前链接仍指向 rc2 时紧急切换命令入口；不会卸载 rc2，Homebrew 安装记录仍同时保留两个版本：

```bash
test -x /opt/homebrew/Cellar/willdeep-cli/0.154.0-rc1/bin/willdeep && test -L /opt/homebrew/bin/willdeep && test "$(readlink /opt/homebrew/bin/willdeep)" = ../Cellar/willdeep-cli/0.154.0-rc2/bin/willdeep && ln -sfn ../Cellar/willdeep-cli/0.154.0-rc1/bin/willdeep /opt/homebrew/bin/willdeep
```

```bash
/opt/homebrew/bin/willdeep --version
```

官网备份目录：`/Users/rocky/.local/share/willdeep/backups/20261119-site-before-1.12.0-rc14-20261119-22544-62u3xy`。保存了上线前 rc13 的首页、所有链接的 JS/CSS chunk 及五份 CLI 文档共 12 个文件，以及之前三个生产源码版本 rc13/rc11/rc10 的 gzip 归档；不把源码快照称为三个完整二进制镜像，也不声称镜像了 benchmark/media 大文件。摘要见[官网备份 JSON](website-backup.json)。

Railway 可选择原 rc13 生产部署 `4be80f46-de09-4dd7-b28e-d6c4f54f45f2` 恢复。需要通过 Git 恢复时，在官网 checkout 建立独立分支，回退本次 merge，审阅后通过 PR 合入 main；不强推、不移动 tag。以下分支名需尚未占用，操作前确认 worktree 干净：

```bash
git -C /Users/rocky/Sites/willdeep-react-static/.claude/worktrees/cli-0154-rc2-site-20261119 fetch origin main
```

```bash
git -C /Users/rocky/Sites/willdeep-react-static/.claude/worktrees/cli-0154-rc2-site-20261119 switch -c rollback/site-1.12.0-rc14 origin/main
```

```bash
git -C /Users/rocky/Sites/willdeep-react-static/.claude/worktrees/cli-0154-rc2-site-20261119 revert --no-commit -m 1 d7f6e488ea4a03e6a902b726cdbecb7f51e82157
```

```bash
git -C /Users/rocky/Sites/willdeep-react-static/.claude/worktrees/cli-0154-rc2-site-20261119 commit -m "revert: restore website before 1.12.0-rc14" -m "Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

```bash
git -C /Users/rocky/Sites/willdeep-react-static/.claude/worktrees/cli-0154-rc2-site-20261119 push -u origin rollback/site-1.12.0-rc14
```

随后在仓库创建并审阅此分支到 main 的恢复 PR，再合并触发部署。恢复后重新核对版本头、页面与资源；不将 Git 操作成功当作线上恢复成功。已打开的旧客户端需要重新启动，替换磁盘文件不会改变现有进程。

## 历史验证与已知边界

规则扫描专项 16 项通过；Core 扩展验证 1361 项通过、7 项忽略、1 项长退避排除；Runtime 67 单元与 1 文档测试、CLI 相关 15 项、CLI Web 52 文件 432 测试通过，严格 Clippy、格式与版本检查通过。npm 安装契约 7 项 38 断言、发布契约 14 项 94 断言通过。以上具体覆盖见[源码验收](project-rule-scan.md)，不将主动中止的长退避尝试计为通过，也不声称本机完整重跑了 Rust 全工作区。

正式归档核对初期经历 GitHub 连接中断、本地 Ruby 输出编码和直接下载超时；修正 UTF-8 解码并使用既有网络代理后完整通过。普通 npm 查询曾读到旧缓存，以强制在线查询及实际安装消除歧义。官网最终完整测试明确使用 Yarn 4.12.0；此前从 CLI 工作目录启动的一次执行实际使用 Yarn 1，不混算工具版本。文档规模检查也曾因默认 US-ASCII 失败，使用显式 UTF-8 locale 后通过，没有修改检查脚本。
