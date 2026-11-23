# CI 对纯文档改动跳过编译

日期：2026-11-23。候选版本：0.154.0-rc12。既有项目迭代，未安装、未公开发布。

## 问题

一次只改文档的 PR 会触发两套完整 CI（push 与 pull_request 各一套），每套含 macOS、Ubuntu 的完整测试，两个 Linux 架构的 `cargo test`，以及 WSL ABI 冒烟。这些 job 都要先构建前端再编译整个 workspace，而文档改动不改变任何构建或测试结果。

不能用 `paths-ignore` 直接跳过。develop 的分支保护把 `test (ubuntu-latest)`、`test (macos-latest)`、`Linux x86_64-unknown-linux-gnu`、`Linux aarch64-unknown-linux-gnu`、`WSL ABI smoke test` 列为必需检查，而 GitHub 不把 skipped 状态算作通过，跳过后 PR 会永远合不进去。

## 决策

路径判断放在可复用 action `.github/actions/ci-changes`，内部用 `dorny/paths-filter@v3`。代码路径为：`crates/**`、`web/**`、`npm/**`、`integrations/**`、`examples/**`、`scripts/**`、`.github/**`、`Cargo.toml`、`Cargo.lock`、`rust-toolchain*`、`deny.toml`、`clippy.toml`、`rustfmt.toml`、`.rustfmt.toml`。其余路径（`docs/**`、markdown、`bench/**`）视为文档。

三个重 job 保持原名、保持无条件运行，各自第一步调用该 action，后续步骤在 `code != true` 时跳过。job 不能整体带 `if`：分支保护按名字要求这五个检查，而 GitHub 不把 skipped 算作通过，整体跳过会让纯文档 PR 合不进去。这样也不新增 job，CI 调度契约测试锁死的 job 名单保持不变。

无法证明 diff 只含文档时保守地跑全量：`workflow_call` 复用拿不到 base ref，分支与默认分支分叉时范围不可靠。同时包含代码与文档的改动走原路径。`release.yml` 由 tag 触发，发版仍全量编译，不受影响。判断依据是 diff 的路径而不是内容：代码文件里只改注释也算代码。

## 验证边界

本决策的验证就是合入它的 PR 本身：该 PR 改了 `.github/**`，属于代码路径，应跑完整 CI；合入后的下一次纯文档 PR 应看到五个检查都成功，但各自在检测步骤之后不再编译。本地 `actionlint` 通过，`scripts/test/ci_self_hosted_attempt_test.rb` 41 项通过，这不替代 GitHub 上的实际派发。
