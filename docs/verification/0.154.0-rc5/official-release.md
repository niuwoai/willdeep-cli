# 0.154.0-rc5 正式发行验收

日期：2026-11-19。结论：`v0.154.0-rc5` 已公开发布，GitHub Release、npm 官方源与 Homebrew 配方全部指向该版本；本机与 q3、q4-pub、h2.niuwoai.com 已完成安装。机器可读记录见 [official-release.json](official-release.json)。

## 发行源

| 项目 | 值 |
| --- | --- |
| 合并提交 | `959a761150823bac96e17cab2c2d744010e7e3d0`（PR [#165](https://github.com/niuwoai/willdeep-cli/pull/165)） |
| tag | `v0.154.0-rc5`（annotated，tag 对象 `a2b82acb4a5f8096b529e86f6862090cf11bd25e`） |
| 默认分支 | `develop` 与 `main` 均为 `959a7611`；`main` 以快进同步 |
| 工作流 | [Release 44507260052](https://github.com/niuwoai/willdeep-cli/actions/runs/44507260052)，全部成功 |
| 发布时间 | 2026-11-19T11:08:50Z |

Release 工作流中的 macOS universal、Linux amd64/arm64 构建、WSL ABI、三个测试任务以及 GitHub Release、npm 发布任务全部为 success。

## 产物

[Release 页面](https://github.com/niuwoai/willdeep-cli/releases/tag/v0.154.0-rc5)：

| 产物 | 大小 | SHA-256 |
| --- | ---: | --- |
| `willdeep-macos-universal.tar.gz` | 46,199,759 | `3ec8da25f6015e4f95e15775fe287ea49fc46a24eb101a8232d9eef6a9e5c0c5` |
| `willdeep-linux-amd64.tar.gz` | 23,332,076 | `4d5ff60dc4b1e4fadb87eb437bf98f457520655340d065aec96a1ef6a3c6e5fd` |
| `willdeep-linux-arm64.tar.gz` | 23,150,596 | `ce9d70610e2b9e35857b9fecdca8b128c72cbad3bcf47bafe037d05df4e1d8de` |

本机经系统代理下载 Linux amd64 产物并复核 SHA-256，解包后的二进制摘要为 `4f20f7c9991f24d0ffd61c4f6689c73de33776f156608e420baad204ab84c2eb`，版本 `willdeep 0.154.0-rc5`。分发到 q3、q4-pub、h2 的 tar 包与二进制摘要均与上表一致。

## 渠道

- **npm 官方源**：`@niuwoai/willdeep` 的 `rc` 与 `latest` 均为 `0.154.0-rc5`（`npm view --registry=https://registry.npmjs.org`）。本机默认 npm 源为 `registry.npmmirror.com`，验证时该镜像尚未同步，属镜像延迟，不代表发布失败。
- **Homebrew**：`niuwoai/homebrew-tap` 提交 `b1ffde1 willdeep-cli 0.154.0-rc5`；本机 `brew list --versions willdeep-cli` 为 `0.154.0-rc5`，`/opt/homebrew/bin/willdeep` 版本 rc5，SHA-256 `1b6787b2a2bbac83f35b0e386a199f64a7b0f4447cb75ee5a5364bd0e845f11c`。
- **官网**：`willdeep-react-static` 的 `src`、`content` 中没有写死的 CLI 版本号或下载地址，CLI 入口指向 GitHub Releases，本次无需改动。

## 边界

- 发行验证来自工作流结果、Release 产物摘要与本机下载复核；arm64 产物未在本机下载复核，其摘要来自工作流日志。
- npm 只验证官方源；镜像同步时间不在本记录内。
- 未使用真实模型请求；安装与健康检查使用各机私有 Runtime。

## 回滚

应用回滚以各机保存的上一版本二进制为准，见 [本机](local-installation.md)、[q3](q3-installation.md)、[q4](q4-installation.md)、[h2](h2-installation.md)。npm 与 Homebrew 的版本标签如需回退，应重新指向上一发行版本，不能仅删除 tag；本轮未执行渠道回滚。
