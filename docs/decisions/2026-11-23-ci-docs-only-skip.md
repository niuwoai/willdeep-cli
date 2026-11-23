# CI 对纯文档改动跳过编译

日期：2026-11-23。候选版本：0.154.0-rc12。既有项目迭代，未安装、未公开发布。

## 问题

一次只改文档的 PR 会触发两套完整 CI（push 与 pull_request 各一套），每套含 macOS、Ubuntu 的完整测试，两个 Linux 架构的 `cargo test`，以及 WSL ABI 冒烟。这些 job 都要先构建前端再编译整个 workspace，而文档改动不改变任何构建或测试结果。

不能用 `paths-ignore` 直接跳过。develop 的分支保护把 `test (ubuntu-latest)`、`test (macos-latest)`、`Linux x86_64-unknown-linux-gnu`、`Linux aarch64-unknown-linux-gnu`、`WSL ABI smoke test` 列为必需检查，而 GitHub 不把 skipped 状态算作通过，跳过后 PR 会永远合不进去。

## 决策

`ci.yml` 增加 `changes` job，用 `dorny/paths-filter@v3` 判断 diff 是否触及代码路径。代码路径为：`crates/**`、`web/**`、`npm/**`、`integrations/**`、`examples/**`、`scripts/**`、`.github/**`、`Cargo.toml`、`Cargo.lock`、`rust-toolchain*`、`deny.toml`、`clippy.toml`、`rustfmt.toml`、`.rustfmt.toml`。其余路径（`docs/**`、markdown、`bench/**`）视为文档。

五个重 job 增加 `needs: changes` 与 `if: needs.changes.outputs.code == 'true'`。纯文档时新增 `ci-skip` job，以与被跳过 job 逐字相同的五个名字各报一次成功。GitHub 对同名检查取最新结果，分支保护规则不用改，也不需要管理员权限。

同时包含代码与文档的改动走原路径，行为不变。`release.yml` 由 tag 触发，发版仍全量编译，不受影响。判断依据是 diff 的路径而不是内容：代码文件里只改注释也算代码。

## 验证边界

本决策的验证就是合入它的 PR 本身：该 PR 改了 `.github/**`，属于代码路径，应跑完整 CI；合入后的下一次纯文档 PR 应只看到 `changes` 与五个同名的跳过检查，且能通过分支保护合入。本地用 `actionlint` 校验了工作流语法，这不替代 GitHub 上的实际派发。
