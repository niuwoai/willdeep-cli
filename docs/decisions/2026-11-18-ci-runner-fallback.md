# CI 自建 runner 的排队上限与托管兜底

日期：2026-11-18。适用范围：既有 CI 的 macOS / Linux 主测试调度。版本与发布记录由主改动同步维护。

## 问题

GitHub Actions 的 job `timeout-minutes` 从 runner 接单执行后开始计时。runner 离线时 job 留在队列里，不产生 failure；依赖 `needs` 失败的兜底 job 因而不能保证及时启动。查询 runner 是否 online 只能反映查询时刻，也无法保证派发后会被接单。

## 决策

保留原来的 `test (macos-latest)`、`test (ubuntu-latest)` 检查名，让它们始终运行在同平台的 GitHub 托管 runner 上，承担协调与最终测试门禁。

只有本仓库非 fork 的普通分支 push、且工作流名为 `CI`、调用者 `github.workflow_ref` 精确为当前分支的 `.github/workflows/ci.yml` 时，协调者才用当前 job 的 `GITHUB_TOKEN` 派发独立的 `.github/workflows/ci-self-hosted.yml`。PR、tag push、Release 调用的验证和其他复用调用直接执行托管测试。worker 工作流仅接受 `workflow_dispatch`，只有一个选定平台的自建 job，不递归派发。主 `ci.yml` 保留原有三个事件与全部主检查，不为 worker 创建同名的跳过检查；worker 检查名也与最终平台检查不同。fork 仓库的 push / dispatch 不进入自建路径。

macOS 自建标签仍为 `self-hosted, macOS, X64, imac`，Linux 仍为 `self-hosted, Linux, X64, q4-pub`。本改动不修改 runner 服务、注册、发布平台或发布依赖门禁。

完整的原主测试抽到 `.github/actions/ci-test/action.yml`，自建与托管调用同一份 suite，保留所有原命令、`runner.os` 条件、工作目录、SDK 与 npm 路径。新增 Linux 离线协调器契约测试。共享 action 不改变工作目录基准。

## 排队与执行预算

- 协调者用单调时钟，从发出 dispatch 前开始给接单等待计 300 秒。工作流 run 变为 `in_progress` 不表示已接单：必须观察到精确 worker job 的 `in_progress` / `completed`。
- 观察到接单后，另计最多 3600 秒执行预算；worker 本身也有 60 分钟 job 超时。
- 正常轮询每 30 秒一次。按每轮读取父 run、子 run 和 worker jobs 三次估算，双平台完整等待 65 分钟约 780 次读取，给每仓库每小时 1000 次的默认 `GITHUB_TOKEN` 配额留出余量。同仓库其他运行也共享配额，真实限流仍须转托管并如实报告清理状态。API 每次请求有 5 秒连接、10 秒读写和 25 秒总调用上限；达到预算后不再继续等待接单或执行，进入清理与托管重跑。
- “5 分钟”是自建接单预算，加上 API 响应、有限的取消确认与下一个步骤调度延迟，不是从 push 起保证 5 分钟完成切换。托管协调者自身排队也不计入这 5 分钟。
- 父 job 为 135 分钟，协调步骤为 70 分钟，给自建等待、执行、API 和清理留余量。**托管完整 suite 的步骤仍单独限制为 60 分钟**，没有把一次托管测试的执行期限扩大为 135 分钟。

托管协调者在等待自建期间同样占用 GitHub 托管 runner，产生相应使用量。这里的目标是调度确定性与保留自建首选，不承诺节省托管分钟数。

## 成功证据与最终门禁

每次请求包含父 run ID、父 attempt、平台和随机 nonce，写入精确 `run-name`。worker 在 checkout 前检查输入格式、branch ref 和 `github.sha == expected_sha`，再以 `persist-credentials: false` checkout 精确 SHA。

协调者同时核验 run 的 repository、event、workflow path、head branch、head SHA、run attempt、唯一 display title，以及精确 worker job 的名称、run ID、SHA、attempt、自建 labels、runner 身份与 conclusion。只有必要步骤（初始化、身份验证、checkout、完整 suite、唯一末尾成功 marker）全部成功，且返回的所有步骤均为成功或合法跳过，才输出 `self_hosted_passed=true`。marker 没有 `always()` 或 `continue-on-error`，无法越过失败的 suite。

worker job 设置 `continue-on-error: true`，使一次会由父检查重试的失败不独立阻断最终 CI。因此 run 的总体 success **不等于测试通过**，协调者不能只看它。自建失败、超时、证据缺失、脚本异常、API 错误或权限不足，都必须让父 job 重跑同平台的完整托管 suite。只有该 suite 真正通过，最终平台检查才通过；托管测试失败保持失败。

失败不会被从记录里抹掉：独立 worker 的步骤和日志仍在，父 job summary 写明首次尝试结果、取消是否确认，并链接到 worker。真正的产品测试失败也会在托管上重跑整套测试；这可能暴露自建环境差异或偶发性，不能将托管重跑通过理解为首次故障不存在。

## API 兼容、权限与安全清理

支持 dispatch 新接口的 HTTP 200 `workflow_run_id`，也支持 HTTP 204 的旧行为；204 通过 `workflow_dispatch + head_branch + 唯一 display_title` 有界发现 child。只查询当前分支最近 100 个 dispatch，找不到时仍受接单预算约束，不等待 GitHub 的长队列期限。异常一律转托管，不要求额外 PAT，也不读取或创建 secret。

根权限只有 `contents: read`；只有父 `test` 协调 job 请求 `actions: write`。Release 的 `verify` 需显式给其复用调用允许该权限，否则 GitHub 会拒绝调用；实际 Release 路径不派发 worker。fork PR 即使声明该权限，条件仍不允许派发。

队列超时、执行超时、API 异常或父取消时，协调器尽力清理自己的 child。每次 cancel / force-cancel 前重新核验完整 run 身份；不会取消本父 run、其他 run、其他平台、其他 nonce、rerun 或 HEAD 已变化的 run。普通取消无效时升级 force-cancel，普通取消返回 409 时也重新核验后才升级；清理本身有界。父取消采用单独的 5 秒总预算，覆盖身份复核、force-cancel 请求与完成确认；立即重新核验后请求 force-cancel，不进入普通清理的轮询等待；重复 TERM / INT 不打断已经开始的同一次有限清理。脚本也在轮询中检查父运行是否结束。

无法承诺 API 不可用、权限不足、派发成功但响应丢失且尚未可发现，或协调者被强行终止时仍能取消成功。未确认的清理明确记为未确认，不会报“取消成功”；父平台检查仍以托管测试作最终门禁。HEAD 在 dispatch 时变化将安全转托管，身份不符的 run 不会被误取消。

## 默认分支引导限制

GitHub 要求默认分支先存在独立 worker 的 `workflow_dispatch` 注册，分支版本才可被 API 派发。新文件 `.github/workflows/ci-self-hosted.yml` 合入默认分支前，GitHub 可能返回 404 / 422；协调者直接走托管，并在 summary 提示默认分支注册后才启用自建优先。不能用额外 PAT 或修改 secret 规避这一平台限制。

## 验证边界

`scripts/test/ci_self_hosted_attempt_test.rb` 使用 Minitest，注入 API、单调时钟和 sleeper，不依赖真实远端，覆盖成功、无 job 排队、已接单失败、初始化失败、marker 缺失、执行超时、403 / 5xx、204 发现、错误身份与 SHA、不误取消其他 run、父取消、force-cancel、引导期与托管门禁结构。`--report-dir target/ci-runner-contracts` 输出 JSON 和 Markdown 契约报告。

本地使用 `actionlint` 校验主 CI、独立自建 worker 与 Release 三份工作流，并核对原主测试抽取前后的命令与条件。离线测试不等于已在 GitHub 派发验证，也不替代 macOS / Linux 的完整产品测试。
