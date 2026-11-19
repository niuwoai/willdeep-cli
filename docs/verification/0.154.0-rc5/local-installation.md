# 0.154.0-rc5 本机安装验收

日期：2026-11-19T11:45Z 前后。结论：本机 cargo 与 Homebrew 两处 CLI 升级到 `0.154.0-rc5`，Runtime 完成 rc2 → rc5 交接；替换前旧二进制已备份并记录反向命令。机器可读记录见 [local-installation.json](local-installation.json)。

## 版本

| 位置 | 安装前 | 安装后 | SHA-256 |
| --- | --- | --- | --- |
| `~/.cargo/bin/willdeep` | 0.154.0-rc2 | **0.154.0-rc5** | `488872874cd812ed8011028491396dbca69dd8de109597fa8e685882ba1fe377` |
| `/opt/homebrew/bin/willdeep` | 0.154.0-rc2 | **0.154.0-rc5** | `1b6787b2a2bbac83f35b0e386a199f64a7b0f4447cb75ee5a5364bd0e845f11c` |

- cargo 版由本 worktree 以 `cargo install --path crates/willdeep-cli --locked` 构建；构建树与发行提交 `959a7611` 的 `git diff` 为空。
- 自 rc3 起 Web 源码未变化，`web/dist` 资源名与内容保持一致；本次没有新增前端业务改动。
- cargo 构建时出现 `clipboard-win v5.4.1`、`chacha20 v0.10.1` 在 registry 中已撤回的 `--locked` 警告；锁文件未修改，安装成功。该警告与 rc2 发行时的既有锁定依赖撤回一致。

## Runtime

- 安装前：`0.154.0-rc2`，PID 61794，`127.0.0.1:55954`。
- `willdeep daemon upgrade` 完成交接：`0.154.0-rc2 -> 0.154.0-rc5`，新 PID 34654，`127.0.0.1:51471`；没有活动工作被丢弃。
- 交接后 `daemon status` 报告 `version=0.154.0-rc5`。

## Homebrew

- 配方由发布 skill 脚本更新：`willdeep-cli 0.154.0-rc2 -> 0.154.0-rc5`，tap 提交 `b1ffde1`。
- 首次 `brew upgrade` 卡在镜像队列；改用 `HOMEBREW_NO_AUTO_UPDATE=1` 直接升级成功，最终 `brew list --versions willdeep-cli` 为 `0.154.0-rc5`。
- 下载经系统代理完成，没有修改系统代理或凭据。

## 备份与回滚

备份目录：`~/.willdeep/backups/willdeep-0.154.0-rc2-20261119-183829/`。

| 文件 | 版本 | SHA-256 |
| --- | --- | --- |
| `cargo-willdeep` | 0.154.0-rc2 | `72a55e6d15b9c227d81428c4f185a4ff3be7fa746057d40177bd7ae5e44cd05c` |
| `homebrew-willdeep` | 0.154.0-rc2 | `64f473f5bd1298f394e2117290d355e39d5e92515ce6fa45e20f9f5d29a5dd54` |

回滚命令已写入同目录 `metadata.json`：分别把对应备份复制回 `~/.cargo/bin/willdeep` 与 `/opt/homebrew/bin/willdeep`；Runtime 需要时再用恢复后的二进制执行一次 `daemon upgrade`。本轮未执行回滚。

## 边界

- 本机 cargo 版不是 Release 下载产物，而是与发行提交同树的本地构建；服务端安装使用官方 Release 产物。
- 未使用真实模型请求；未修改 provider 配置、密钥或系统代理设置。
- npm 官方源已为 rc5，本机默认镜像 `registry.npmmirror.com` 尚未同步，与本地二进制无关。
