# 重复上下文压缩核查

日期：2026-11-07。版本：`0.147.0-rc4`，未发布、未安装。

## 基线与范围

核查时远端 `develop`、worktree HEAD 和正式 tag `v0.147.0-rc3` 均解引用到 `ec565d68657eeaf97e0ffc2467c7bd6b49a65e3d`。在独立 worktree 中使用合成消息、临时工作区、内存 Provider mock 和本地 `read_file` 复现；未调用真实付费 Provider，未读取生产任务提示词、工具正文、日志正文或凭据，未操作生产 Runtime。

正式源码已经包含摘要复用、增量摘要、durable/projected 索引映射与跨运行缓存。旧进程的实际二进制与正式安装不同，单凭旧进程现象不足以判断正式版本；以下结论来自正式源码的独立本地复现，不能据此证明生产任务的唯一根因。

## 根因

`Agent::run_inner` 每轮刷新 system，并无条件追加当前/剩余模型轮次；时间、Token 预算、目标和计划也可改变。`SummaryCache::matches` 对完整 system 消息计算指纹，下一轮预算计数变化使缓存不匹配。只要分页后的原始投影仍达到水位，就重新摘要旧前缀，绕过既有的摘要复用和增量分支。

历史定位：动态轮次预算提示由 `bd3c102b385e2002a4e2bf95dcfca704a8800452` 引入；当前完整 system 指纹比较由 `f7cbcf8314e0e0b053a12c6d51598a44c25289c2` 引入。两者均已包含在本轮核对的正式 tag 中。

Runtime 正常成功路径已经传入、返回并保存摘要。同一 Agent run 也一直传递同一缓存槽；本次可复现原因是校验输入每轮变化，并非缓存对象每轮重新创建。已有直接调用 `request_messages` 的测试未覆盖宿主重建 system，单轮 run 接单轮 run 也不足以暴露它。

## 修复

- 稳定系统提示、项目规则、验收契约、实例身份和静态计划契约继续进入摘要原料与 system 指纹。
- durable 消息和检查点保留完整实时 system 提示；只在已有的分页投影中替换成稳定规则，不额外复制整份历史。压缩后向主模型请求回填本轮实时计划、目标、轮次/Token/时间预算；不被摘要，不作为缓存键。
- 非托管摘要器原有的独立目标注入改为静态说明，避免实时目标状态通过另一条路径进入摘要。
- 在分页、水位判断与容量校验前，完整预留实时提示的估算量。实时提示本身耗尽窗口时直接报错，不调用摘要 Provider。
- 仍使用原有 durable 前缀指纹和投影映射，未放宽前缀修改、系统规则修改、越界或损坏缓存校验。旧缓存可因指纹变化安全失效一次，不需要持久协议迁移。

## 可复现证据

正式基线原有上下文测试：

```bash
cargo test --locked -p willdeep-core agent::context::tests -- --nocapture
```

结果：42 passed，0 failed；单测运行 1.94 秒，首次隔离编译 2 分 45 秒。

在仅加入真实多轮测试、未改动运行代码时：

```bash
cargo test --locked -p willdeep-core real_agent_tool_turns_reuse_the_compressed_prefix -- --nocapture
```

失败（退出码 101）：先执行 5 次临时文件读取，第 6 轮结束，再携带返回的摘要进入后续单轮 run。七次主模型请求前，摘要 Provider 累计调用数为：

```text
actual:   [1, 2, 5, 8, 11, 14, 17]
expected: [1, 1, 1, 1, 1, 1, 1]
```

首次摘要只需一次；后续增加来自真实摘要 Provider 调用，不是重复压缩事件的显示误判。该夹具同时检查用户约束、工具配对、最新轮次提示与最终容量。

## 修复后验证

最终定向验证通过，共 86 项（单测数量不重复累计）：

| 命令过滤范围 | 结果 | 单测执行耗时 |
| --- | --- | --- |
| `cargo test --locked -p willdeep-core agent::context::tests -- --nocapture` | 47 passed，0 failed | 2.89 秒 |
| `cargo test --locked -p willdeep-core agent::tests -- --nocapture` | 35 passed，0 failed | 3.97 秒 |
| `cargo test --locked -p willdeep-core agent::guidance::tests -- --nocapture` | 4 passed，0 failed | 0.12 秒 |

五个新增测试覆盖：真实多轮和后续 run 摘要复用、每轮最新工具调用参数与结果保留、历史前缀/系统规则真实修改时失效、计划/目标/截止时间保持完整且不进入摘要、超时结果和检查点保留同一完整 system，以及原历史低于旧水位但加实时提示就超窗的容量边界。复用夹具的首次摘要之后，后续六次主模型请求新增摘要调用为 0；没有把重复事件等同于重复模型调用。

`cargo clippy --locked -p willdeep-core --all-targets -- -D warnings`、`cargo fmt --all -- --check`、`ruby scripts/check_release_version.rb 0.147.0-rc4` 与 `git diff --check` 均通过。最后的事件估算调整另复验上下文测试（47 passed，0 failed，3.96 秒）和严格 Clippy，均通过；[JSON 记录](context-compression.json) 保留命令、结果和未完成项。最终独立静态复核未发现新增正确性问题；超时回归覆盖主模型等待，压缩等待期间的完整 system 仅有源码边界核验，既有取消测试仍验证摘要回退。

早期扩展验证 `cargo test --locked -p willdeep-core agent:: -- --nocapture` 实际匹配 226 项（包含 `subagent::`）。输出显示三个新增测试通过，最后停留在 `subagent::dispatch_store::tests::recovery_inventory_beyond_4096_paginates_within_parent_and_marks_reports`，报告运行超过 60 秒且后续未退出。为验证后续修订，主动停止该命令；不将这一轮记为完整通过。另一次 `cargo test --locked -p willdeep-core --lib` 在编译阶段主动停止，没有测试结果。随后改用精确模块过滤，不改动或跳改该恢复测试。

## 验证边界

没有发布、安装、移动既有 tag 或干预生产任务。无浏览器界面改动，本次验证使用真实 Rust Agent 循环和隔离 mock；不启动 Web 服务。不能以这些本地测试替代各平台 CI 或生产正式构建接管后的观察。
