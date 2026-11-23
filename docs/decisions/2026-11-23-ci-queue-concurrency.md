# CI 同一提交只占一套名额，过期的跑自动取消

日期：2026-11-23。适用范围：`.github/workflows/ci.yml` 的触发与并发。版本与发布记录由主改动同步维护。

## 问题

组织使用 GitHub Free。托管 runner 同时最多 20 个 job，其中 macOS 只有 5 个，而且这 5 个由组织内所有仓库共用。`ci.yml` 对任意 `push` 和任意 `pull_request` 都触发，带 PR 的分支一次提交因此进两次队列，每次 5 个 job。2026-11-23 10:24（UTC）有 25 个 CI run 在排队，其中 `ci/claude-batch-workflow` 一个分支占了 6 个；当天一次已经成功的 `test (macos-latest)` 排队 52 分钟，实际执行 26 分钟。没有 `concurrency`，同一分支上更早的提交即使已被后续提交取代，也继续占着名额直到跑完。

## 决策

`push` 只保留 `develop`、`main` 和 `v*` tag。有 PR 的分支只由 `pull_request` 触发，同一提交不再占两套名额。`workflow_call` 保持不变：`release.yml` 的 `verify` 通过它复用整套 CI，tag 的校验次数与现在一致。

增加 `concurrency`，分组键为 `ci-${{ github.workflow }}-${{ github.event.pull_request.number || github.ref }}`，`cancel-in-progress: true`。同一 PR 的新提交取消该 PR 上还在排队或执行中的旧 CI；`develop`、`main` 和每个 tag 各自成组。`merge_group` 没有 `pull_request.number`，会落到 `github.ref`，将来启用合并队列时不会与 PR 的组撞车。取消只作用于本 workflow 内同组的 run，不取消其他 workflow 的检查，也不取消自建 worker 的独立 run。

不改 job 名称、不给 job 加 `if`。分支保护把 `test (ubuntu-latest)`、`test (macos-latest)`、`Linux x86_64-unknown-linux-gnu`、`Linux aarch64-unknown-linux-gnu`、`WSL ABI smoke test` 列为必需检查，skipped 不算通过。自建 runner 的派发条件、5 分钟排队上限和托管兜底保持原样。

## 后果

往功能分支推送但还没开 PR 时，CI 不再自动跑，开 PR 后才跑。`develop` 与 `main` 的直接 push 仍会跑。被取消的 run 结论是 `cancelled`，不是失败；必需检查以该 PR 最新一次提交的结果为准。已经排队的历史 run 不受本次改动影响，合入后只对之后的触发生效。
