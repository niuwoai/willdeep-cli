# P4a 与 P4b 三仓库免费联合验收

日期：2026-10-15。已完成 P4a 基础和 P4b 免费工程部分；真实模型实验仍待用户确认，不能据此宣称完整 P4b 质量评测或 RSI 提升已经完成。

## 实现范围

| 系统 | 本批版本 | 能力 |
|---|---|---|
| Rust CLI / Runtime / Web | 0.102.0-rc1 | 实际 Worker 证据、独立指令候选、精确 revision 双臂离线 runner、恢复与配额、摘要授权上传、可见实验区、显式同步冲突处理 |
| Mac Xedit | 1.420.0-rc1 / build 918 | 原生前后台与 Goal 证据、同契约 runner、独立候选、授权上传、部分报告、同版本分叉显式解决 |
| some.im | 0.249.0-rc1 | 独立候选和实验域、客户及租户隔离、授权撤回、事务准入和幂等、不可变 client_reported 证据、新增迁移 227 |

三端契约、golden、schema、33 个反例及非空技能 fixture 已逐字节核对一致。候选不移动安装、私有 current 或官方 head；客户端证据不自动升级为可信官方证据。

## 最终免费验收

| 验收 | 实际结果 |
|---|---|
| Rust 本机运行及真实本地 HTTP | 原创 24 题、双臂共 48 个真实 Worker / read_file / host verifier trial，9 项联验全部通过；完成恢复不重复，二次上传幂等 |
| Rust 最终构建 | SHA256 `e1cac01ff1a2aa216dfeedffbe2d8aae163b0d56306bb95b190f7921a21f51dc`；注册表 22 项、最终 Web 8 项、针对 RSI 回归及全目标 Clippy / 构建通过；不同实现快照的完整回归见 [Rust 明细](agent-rsi.md)，不把计数合并为一次运行 |
| Mac 原生 | 最终 54 / 54、零跳过；6 个原生双臂 trial 已实际上传，原生契约读取并绑定 Rust 的 48 条证据 |
| Mac → Rust | 从真实本地客户 API 读回 Mac 最终六条证据，服务回执 canonical SHA 与实际 Rust core 的候选 / Spec / 执行证据绑定均通过 |
| 双端旧注册表 | 真实 Rust CLI 与 Mac 服务层经本地 HTTP 完成四轮同版本不同 SHA 分叉；两端各验保本机新版本和采用云端；另验固定历史、用户 / 租户切换、旧会话拒绝及删除不复活，共 10 项检查 |
| some.im | RSI HTTP 42 / 42，注册表 HTTP 9 / 9；相关 race、vet、前端 7 项及 Linux 构建通过；隔离 MySQL 9.5.0 迁移 up / down / up 和 store 检查通过，自建库清理完成 |
| Web 实际入口 | 最终 JS `index-PVJxb1qO.js`；浏览器实际起草、登记、运行并查看六条报告，完整安装记录前后相同；JSON 详情默认折叠；响应版本 0.102.0-rc1 |
| 原创题目 | 12 个缺陷 / 12 个无缺陷对照，宿主 TypeScript 转译及断言 24 / 24；这些只证明素材及流水线，不是模型答题成绩 |

最终 Rust HTTP 实验为 `rust24-99130f02d2`；Mac 原生读取的另一份同批不可变 Rust 实验为 `rust24-520ea3d21e`。Mac 最终实验为 `exp-36015461-878f-41f4-9df5-6557424eb9e0`；Web 最终实验为 `exp-c320c130-e975-4840-ac22-9a4648a6dd12`。不同实验和二进制指纹分别保留，不能混成一批质量成绩。

## 报告与复跑

- Rust 48 次与 HTTP 的本次报告位于 `/private/tmp/willdeep-rsi-acceptance/rust-release-candidate-final/rust-native-http.json` 及同名 Markdown。
- Mac 汇总：`/private/tmp/xedit-agent-registry/build/rsi-offline-acceptance/acceptance.json`；原生测试详报：`build/fast-tests/report.json`。
- 双端冲突：`/private/tmp/xedit-agent-registry/build/rsi-registry-interop/da3b8ac5-bf5c-41c0-86e8-5e5eef60c9e2/report.json` 及同名 Markdown。
- Mac 到 Rust：`/private/tmp/willdeep-rsi-acceptance/mac-to-rust-final/mac-to-rust.json`。
- Web：`/private/tmp/willdeep-rsi-acceptance/web-verified-final/web-acceptance.json`、`report.png` 及同名 Markdown。
- 服务端：`/private/tmp/someim-rsi-p4ab-delivery.json`；真实 HTTP 及 MySQL 原始记录另见该报告。

可复跑脚本：Rust 的 `scripts/agent_rsi_native_http_acceptance.rb`、`scripts/agent_rsi_peer_evidence_acceptance.rb`；Mac 的 `scripts/agent_rsi_acceptance.rb`；服务端的 `scripts/agent_rsi_service_acceptance.rb`、`scripts/agent_registry_offline_acceptance.rb`。HTTP 脚本只接受隔离 loopback 和显式 mock_only 假会话回执；凭据留在仓库外 0600 文件，不进入报告或提交。

## 交付边界及下一步

真实模型请求和费用均为 0。所有执行为确定性 mock；付费模型及非零付费预算仍被拒绝。原生账号切换验的是实际服务层加合成客户，未声称手动 GUI / 生产账号验收。Web 使用独立 9858 与临时 home，未更新用户 9847。三仓库只创建本地 feature 提交，未推送、合并、打标签、发布或安装。生产未执行迁移 227 或部署。

RSI 质量判断、真实 adapter 的可信预算批准、人工晋升、官方灰度、通道回滚和连续父子迭代仍按后续阶段推进。首个真实模型试验采用 [单独确认清单](../../decisions/2026-10-15-agent-rsi-paid-evaluation-proposal.md)：deepseek-v4-flash、24 题、两端双臂各重复三次，共 288 次计划执行，含基础设施重试最多 360 次，总预算上限 5 美元。用户确认后再完成真实执行准入和正式评分冻结，不能把客户端 JSON 的 approved 当批准。
