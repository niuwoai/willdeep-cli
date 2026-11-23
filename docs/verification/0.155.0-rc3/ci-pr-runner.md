# PR 自建 runner 调度修复验收

日期：2026-11-23。候选版本：0.155.0-rc3。

## 实际原因

[PR 204 的 CI 运行](https://github.com/niuwoai/willdeep-cli/actions/runs/45199061446) 已允许同仓库 `pull_request` 尝试自建，但脚本只接受 `refs/heads/`。PR 的 `GITHUB_REF` 为 `refs/pull/204/merge`，进入校验即返回 `unsupported branch ref; using hosted fallback`，没有派发 worker。Linux job 137777302865 的日志已核实同一错误。

查询时 `willdeep-imac`、`willdeep-q3`、`willdeep-q4-pub` 均 online 且不忙；这次回退发生在 runner 接单之前。

## 修复

- 派发使用真实源分支和源提交，测试目标保留父 CI 的精确合并提交。
- worker 的 `workflow_sha` 绑定派发身份，`expected_sha` 绑定实际 checkout 及完整测试目标。
- child run/job 核验、204 发现和取消继续使用派发身份；源分支移动不得被误认成成功。
- 所有合法 Git 分支名可用，包含中文、加号和字面量 shell 特殊字符；Git 校验通过独立 argv 执行。
- 自建优先、300 秒接单预算、3600 秒执行预算和完整托管兜底保持有效。fork PR、tag 和发布调用沿用托管路径。

## 已执行

- `ruby scripts/test/ci_self_hosted_attempt_test.rb --report-dir target/ci-runner-contracts`：47 runs / 650 assertions，0 failures / 0 errors / 0 skips，生成 JSON 与 Markdown 报告。
- 新回归覆盖两平台的 PR merge/source SHA 分离、HTTP 200/204 派发、真实入口环境变量传递、合法特殊字符分支、源分支移动和 Git 校验退出码。
- 从 worker YAML 取出实际 Ruby 身份校验脚本执行：允许有效的不同 merge/source SHA；拒绝 source SHA 移动、非法 SHA、tag ref 和非法 request ID。
- `actionlint`：主 CI、自建 worker、Release 三份工作流通过。
- `cargo metadata --offline --locked --no-deps --format-version 1`：四个 workspace 包均为 0.155.0-rc3，锁文件可用。
- `git diff --check`：通过。
- 独立只读复核未发现修复引入的实质问题。

## 验证边界

本修复位于隔离分支 `fix/ci-pr-ref`，基于 `ci/pr-self-hosted` 的 0afc77f6；未替换现有 PR，未实际派发修复后的远端 worker。离线验证不代表两平台完整产品测试通过。源分支的 worker 工作流需要包含新增的 `workflow_sha` 输入，旧版入口不兼容时仍回退托管。
