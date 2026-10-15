# 智能体 RSI P4a / P4b

当前版本：0.102.0-rc1。实现范围为工程闭环与免费离线验收；没有真实模型质量结论、人工晋升、灰度或自动发布。

## 执行证据

普通定义 Worker 在组装实际系统提示词、约束工具并解析冻结定义后，记录 `willdeep.worker-evidence.v1`。事件位于运行 home 的 `agent-evidence`，内容只含定义 ID / revision、基础职责、实际提示词和技能摘要、工具集合、根与 Worker 关联、attempt、模型及有效资源限额。开始、完成、中断、独立 verifier 分开保留。完成状态保持 unverified；只有宿主执行的 verifier 能形成通过材料。异常、取消或超时导致 future 被丢弃时保留 interrupted 事件。反馈行同时关联这些血缘。

这些普通事件不伪造 experiment 或数据集引用。只有明确登记的实验才能组装 `willdeep.agent-rsi.evidence.v1` 完整证据。跨平台协议、canonical、explicit null 与 HTTP 接口见 [共享契约](AGENT_RSI_CONTRACT.md)。

## CLI 操作

1. 准备 baseline 与指令候选两个 `agent-definition.v1` 包。候选必须只改变 instructions、version 和 forked_from；父 hash 固定，能力、职责、用途和技能均保持一致。
2. `willdeep agents rsi candidate --baseline baseline.json --candidate candidate.json --id candidate-1` 只存候选。不会调用安装、选择、私有 current 或官方发布 API。
3. `willdeep agents rsi prepare --baseline baseline.json --candidate candidate.json --id experiment-1` 冻结内置三题实验。可加 `--suite bench/agent-rsi/reviewer-original-24-v1/suite.json` 使用 24 题原创验收材料。
4. `willdeep agents rsi run experiment-1 --agent-revision 候选完整SHA256` 直接调用精确 revision，两臂交错执行。`--resume` 只继续同一 manifest / 运行二进制 / gate，已完成样本不会再跑；失联 admission 明确补成 interrupted，保留原尝试并在预算内建立新的恢复段。
5. `willdeep agents rsi report experiment-1` 查看完整 JSON；本机同时保存 report.md。`willdeep agents rsi list` 查看独立实验。`check FILE` 校验共享对象。

本地目录为 `$WILLDEEP_HOME/agent-rsi`，按服务 origin / user / tenant 分账号命名空间，未登录实验单独保存；切换或退出账号不列出原账号实验。候选的本地 `local-only` 标记不是云端授权，上传必须另选当前账号真实有效的 grant。每题、每臂、重复和尝试均独立 workspace、home、反馈和事件。只把该题 fixture.txt 放入 Worker 工作区；预期答案及验证器留在宿主侧。隔离评测不加载实时用户规则、提示词变体、记忆或 Provider 配置。正常 Worker 仍使用原用户规则。当前离线适配器限定只读 reviewer，拒绝写入及进程能力。

suite 格式为 `{suite_id,tasks:[{task_id,split,prompt,marker,fixture,expected_output}]}`。任务、完整 suite、split、fixture 和 verifier 摘要均冻结；技能指纹为完整有序 `definition.skills` 数组的 canonical hash，运行时另核对实际加载内容摘要。三个 split 必须存在，题目最多 128，重复最多 10，attempt 最多 10，并发上限最多 4（当前 runner 串行执行，不超过声明上限）。每个实验一个候选。holdout 在固定账号、授权域与 suite 下跨实验累计，不退款，初始上限只能收紧；所有 validation / regression 的题目、双臂和重复均已通过才开放 holdout，输入题目顺序不能绕过阶段门禁。

候选、Spec 与证据采用原子不可覆盖创建；同 ID 不同内容的并发写只有一方能成功。登记与执行共用操作锁，锁 inode 保留而所有权由操作系统在崩溃时释放。中断恢复必须引用同一样本上一条 interrupted / infra_error 终态，缺失父段或跨样本恢复直接拒绝；通过、失败和预算耗尽均不授权再跑挑分。

## 免费边界与结果含义

当前唯一 Provider 为进程内确定性 Mock，不创建网络 Provider。它通过正常 Worker 的 read_file 工具读取 fixture，再返回宿主提供的离线结果。独立 verifier 检查实际工具成功与报告合同，用于验证血缘、隔离、报告和恢复机制。

Rust verifier 的 `input_sha256` 是 `willdeep.rust-offline-verifier-input.v1` 对象的 canonical hash；对象包含实际 `report_sha256`、宿主预期结果 `expected_output_sha256`，以及按实际顺序保留、参与判定的 `tool_completed` 事件。它不使用题目 hash 代替验证输入。该对象保存在本机 events 中，恢复时重新核对输入摘要及实际报告摘要。未执行的孤儿段使用独立 `willdeep.rust-offline-verifier-unexecuted.v1` 输入摘要并保持 unverified。

`offline_pipeline_passed` 只代表工程流水线通过，`quality_improvement_claimed` 和 `official_promotion_allowed` 永远为 false。样本按 task / arm / repetition 计数，所有 attempts 均保留，未知用量不补零。真实模型、付费预算、未登记路由直接报 paid_not_approved；在用户另行批准模型、题量与预算前，不会运行真实评测。

后续真实定义级 adapter 需要独立评审器 / 可执行 oracle、配对统计和资源门禁；不能把离线 prefix verifier 当质量指标，也不能用 Rust 成绩替代 Mac 验证。

## Web 与服务端

### 同步冲突的明确解决

`willdeep agents sync` 除冲突 ID 外还返回 `conflict_details`，包含用户看到的本机 revision、云端 ETag、两端版本及云端删除状态。Web 显示冲突卡片和两个操作按钮，原始 JSON 不再是唯一处理流程。

`willdeep agents resolve user/reviewer --choice keep-local --expected-local-revision 本机SHA --expected-remote-etag 云端ETag --new-version 1.0.0-rc3` 保留本机内容并上传。若两边以相同 version 保存了不同内容，必须明确提供未使用的新版本；界面给出可编辑建议值。新包仅改变 version，原两份包均保留在历史中，成功响应提供新 version 和 revision。服务端的同版本不可变约束不放宽，409 / 412 不自动强行重试。

`--choice use-cloud` 采用已确认的云端状态，无须新版本号；云端已删除时移除本机安装，历史包仍保留。固定版本仅在此明确选择后才可换成云端内容，并继续保持固定；本机 Provider / model 绑定和停用状态保留。Keep-local 可用 tombstone 的 ETag 明确恢复云端条目。

Web 解决请求必须携带显示冲突时的 expected_account（origin / user_id / tenant_id），Runtime 在读取任何智能体资产或联网前立即与当前客户会话比较，防止点击到排队执行之间切账号而改变动作指向；CLI 同样携带调用开始时的账号快照。通过后继续使用同一会话，所有步骤在每次客户身份 / 目录读取及写请求前后检查；切账号取消。本机最终安装或删除在与普通编辑、导入、固定和回滚共用的 account-library 存储锁内检查 expected revision，避免检查后被新编辑夹入。保留本机用已确认 ETag 做服务端 If-Match，采用云端会重新核对同一 ETag；两端变化需重新同步后选择。

Web 智能体库提供实验区：从当前包起草专业指令、仅保存候选、登记免费配对实验、运行与查看报告。保存和登记不触碰当前安装，界面三种语言走资源字典。

`willdeep agents rsi upload experiment-1 --grant-id 已单独批准的授权ID` 才会使用 some.im 客户会话登记基线 / 候选定义并上传执行摘要。它固定开始时的账号和缓存 namespace，核验当前客户 user / tenant，检查有效授权，再登记独立 candidate、experiment、admission 与不可变 evidence；每次 HTTP 前后复核会话，账号切换就取消。不会用推理 API Key 代替客户身份。执行摘要不含题目、源码、路径、命令或输出正文；服务端强制将 trust 标记为 client_reported，本机保留原始 local_mock 与服务回执。登录或已有定义同步不代表授权上传实验材料。上传先发全部非 holdout 记录，再按样本及 attempt 保留恢复父段顺序，不会因本地目录顺序先读取到 holdout 就越过服务端门禁。

生产迁移、部署、安装、发布、官方通道和付费执行均不属于本次实施。

## 联合验收与复跑

三仓库最终免费结果和证据路径见 [P4a/P4b 联合验收](verification/0.102.0-rc1/p4ab-integration.md)。Rust 的 `scripts/agent_rsi_native_http_acceptance.rb` 可复跑原创 24 题双臂及真实本地 HTTP；`scripts/agent_rsi_peer_evidence_acceptance.rb` 使用实际 Rust core 校验 Mac 上传证据。两者必须显式传入仓库外的 `RSI_TEST_RECEIPT`，且拒绝非 loopback 或未标记 mock_only 的服务。

同步返回冲突后，CLI 使用 `agents resolve ID --choice keep-local|use-cloud --expected-local-revision SHA --expected-remote-etag ETAG` 明确选择。双方同版本不同内容时，保留本机必须增加 `--new-version`；原两份包保留历史，服务端同版本不可覆盖。Web 显示两种选择和可编辑新版本，Runtime 同时验证点击时账号、云端 ETag 及锁内本机 revision，任一变化要求重新同步。
