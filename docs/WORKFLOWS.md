# 声明式工作流

> 最后更新：2026-11-04 | 当前版本：v0.141.0-rc2
>
> 工作流定义：WorkflowSpecV1（`version: 1`）| Runtime 协议：1.0

## 本轮范围与状态

本轮把声明式图、持久运行、父会话安全派工和 Web/TUI 控制接入现有 Runtime。工作流解释器不执行任意 JavaScript，不增加模型厂商 SDK，也不另建模型路由或消息系统。专业智能体定义仍用原 v1 交换包，工作流定义与本机运行状态分别存储。

本轮已实际执行 Rust workspace 测试、Clippy、Web 测试与构建、隔离 Runtime/API/E2E 及真实宽窄 PTY 验收。以下说明实现契约，文末矩阵对应已运行的证据；完整命令、结果、浏览器检查和未覆盖的平台见 [验收报告](verification/0.141.0-rc1.md)。

## 定义、节点与依赖

`WorkflowSpecV1` 包含 `version`、`id`、`name`、可选 `input_schema`、`phases`、`nodes` 与 `output`。`workflow.schema`（或 `workflow_read` 的 `schema`）返回 DSL Schema 和最小示例。定义 ID 对应不可变内容：同 ID 同内容可重复保存，修改内容必须使用新 ID。run 保存完整定义、输入与 limits 快照，不随之后的目录编辑改变。

| kind | 必需字段 | 执行与输出 |
| --- | --- | --- |
| `agent` | `profile`、`prompt` | 唯一调用模型的叶节点。`input` 解析为数据交给 Worker；输出为独立 JSON 值，可用 `output_schema` 校验 |
| `switch` | `condition`、`then_scope`、`else_scope` | 只展开选中的分支，发布该分支 scope.output；未选分支不派工 |
| `foreach` | `items`、`body` | 冻结数组与顺序，每项建立独立子图，某项完成便可推进其自身后继；按原项序汇总 body.output |
| `repeat` | `initial`、`body`、`until`、`carry`、`max_iterations` | `previous` 初始为 initial；每轮结束用该轮结果及旧 previous 计算 until/carry，新 carry 作为下一轮 previous，条件满足时成为节点输出；耗尽上限仍不满足则失败 |

节点共享 `id`、可选 `depends_on` 与 `phase`。子 scope 为 `{nodes, output}`；每个作用域内部必须是 DAG，顺序和并行完全由依赖关系决定。phase 只分组展示，**不构成阶段等待屏障**。同 scope 不允许重复 ID、自依赖、缺失依赖、依赖环或引用不可见分支。只有 repeat 允许有界重复。

agent 可选 `worker_tier`（standard/advanced/expert）、`max_turns`、`token_budget`、`timeout_seconds`、`write_targets`、`task_packet`、`escalation`、`target_command`。expert 的 `escalation` 使用现有升级票据字段 `reason/attempted_profiles/context_evidence/why_not_decompose`，仍由父会话核验调查证据和预算；`target_command` 仅供 ops_runner，并须单独授权。这些字段只向既有安全派工链传递限制与任务，不授予额外权限。profile 可为基础职责或已安装且可用的专业 ID，模型解析继续使用现有 Provider、档位及本机绑定。共享调度和快照规则见 [子 Agent](SUBAGENTS.md)。

## 模板引用与条件

模板是普通 JSON 值。引用必须是只有一个键的对象，例如 `{"$ref":"input#/items"}`，不会在 prompt 字符串内做任意插值。

| 引用根 | 可见范围 |
| --- | --- |
| `input#` / `input#/path` | 本次运行输入及其 JSON Pointer 子值 |
| `item#` / `item#/path` | 当前 foreach 项，仅 foreach 及其继承作用域可见 |
| `previous#` / `previous#/path` | repeat 当前轮开始时的 carry，仅对应循环作用域可见 |
| `node:ID#` / `node:ID#/path` | 当前节点声明的传递依赖及继承的可见上游；scope.output 可读取本 scope 节点 |

指针遵循 RFC 6901，空指针读取整体，`~0` 表示 `~`、`~1` 表示 `/`。缺失指针、引用对象有其他键、错误转义、不可见根或跨分支引用明确报错；动态缺值错误包含节点路径，不静默替换成 null。

条件操作仅支持 `eq/ne/lt/le/gt/ge`（left/right）和 `and/or`（非空 conditions）、`not`（condition）。比较要求相同标量类型；大小比较仅数字或字符串，布尔值与 null 只做相等/不等。无隐式类型转换，不提供脚本、正则或自定义表达式执行。

## 结构化结果与有限 Schema

Worker 的 JSON `output` 与人可读 `report` 分存。宿主解析并完成 Schema、大小检查后才发布给下游；Worker 自述成功或文本报告不能代替结构化校验，也不依赖某家 Provider 强制调用工具。结果校验失败持久记录为节点失败，依赖节点阻塞；独立分支可收尾，实际在途任务收拢后再形成整体终态。

输入和 agent 输出 Schema 接受布尔 Schema 或有限对象 Schema：

- 注释：`$schema`、`title`、`description`；`$schema` 只接受实现列出的 draft-07/2020-12 URI，不触发联网。
- 类型/值：单一 `type`（null/boolean/object/array/number/integer/string）、`enum`、`const`。
- 对象/数组：`properties`、`required`、`additionalProperties`（布尔或子 Schema）、`items`（单一子 Schema）、`minItems/maxItems`。
- 字符串/数字：`minLength/maxLength`、`minimum/maximum`、`exclusiveMinimum/exclusiveMaximum`。

未知关键字、类型联合、`$ref`（包括本地和远程）、`pattern`、`format`、`oneOf/allOf/anyOf` 等均拒绝。这里的模板 `$ref` 与 JSON Schema 的 `$ref` 是两种语义；用于描述 DSL 本身的 `workflow.schema` 可以含 Schema 组合关键字，不表示用户输入/输出 Schema 也支持它们。

## 硬限与共享调度

WorkflowLimits 默认值是协议绝对上限，运行只能降低，不能提高；超限返回错误，不截断任务。字节限制按 UTF-8/序列化 JSON 计算。

| limits 字段 | 默认上限 |
| --- | ---: |
| `max_definition_bytes` | 1 MiB |
| `max_run_bytes` | 32 MiB |
| `max_nodes` | 256（含嵌套定义节点） |
| `max_depth` | 16 |
| `max_instances` | 4096（动态展开实例总数） |
| `max_attempts` | 4096（本 run 新准入叶派工总数，不含复用结果） |
| `max_foreach_items` | 256 |
| `max_iterations` | 128 |
| `max_concurrent` | 16（单图未完成叶意图上限） |
| `max_value_bytes` | 1 MiB |
| `max_report_bytes` | 64 KiB（单叶报告） |
| `max_name_bytes` | 128 |

额外固定限制：JSON 嵌套深度 64，条件及 Schema 嵌套深度 32，单个写目标路径 4096 字节。定义、phase 与节点标识只接受有界 ASCII 字母、数字、`_`、`-`、`.`，不能为 `.` 或 `..`。聚合报告受整体 run 大小约束，不等同于单叶报告上限。

单图的 `max_concurrent: 16` 不保证 16 个 Worker 同时运行。所有工作流和普通 Worker 共用父会话 Scheduler，现有默认值为 **8 并发 + 64 排队**，可在 `[agent_team]` 调整；driver、switch、foreach、repeat 不占 Worker 槽。现有审批、基础职责、精确写集合、expert 升级票据、团队预算、文件租约、单层团队限制对每个叶节点同样有效。

## 持久生命周期、停止与恢复

运行位于 `$WILLDEEP_HOME/workflows/<parent_session_id>/`，定义在 definitions、运行在 runs。文件锁保护状态/意图事务，使用私有文件、临时写入和原子替换；同父会话执行权由现有会话执行器及锁协调。工作流不另开网络服务，凭据不进入定义或执行快照。

1. run 准入固定 spec/input/limits；运行 request UUID 同时作为 run ID。同 request 同参数返回原运行，参数不同拒绝。
2. 引擎先保存节点实例、路径、foreach 索引/repeat 轮次、attempt/request ID 与派工意图，再由 driver 调用共享派工入口。重复轮询未绑定意图使用相同 request ID。
3. 重启先按 request ID 对账 AgentRun，补齐已接受但尚未回写关联的窗口；成功结果收集一次。不能确认副作用结果时标为 `interrupted`，等待明确恢复，不能凭没有回执就盲目重放。本契约不承诺跨进程 exactly-once。
4. stop 将运行置为 `stopping`，请求取消所属 Worker；只有真实任务终态被收集后才收敛。停止回执不代表所有命令已经退出，不确定项必须保留中断信息。
5. retry/resume 要求源运行已经终结，创建关联新 run；原 run 终态与历史不改写。新 run 带 `source_run_id`，保留成功输出并以 `reused_from` 标记来源；复用不产生新模型调用或执行用量。原图和输入保持不变，不能把任意其他定义/输入的旧成功拼进本次运行。

首次叶派工前，宿主冻结整条运行的专业目录、Provider/model 路由、档位和执行限制；retry/resume 沿用源快照。凭据不进快照，恢复时由宿主重新解析并核验原绑定，当前权限/预算另行核验。普通 Worker 的 continue/recovery 同样使用原派工快照。

工作流节点只能通过所属运行整体停止、重试或恢复；普通 Agent 的续做、重试、发指令和停止入口拒绝修改它。父执行宿主释放时先持久停止当前宿主已接管的活动图，再取消全部所属 Worker；外部新受理而尚未接管的运行保留给后续控制轮次，不受界面最近 50 条的展示窗口限制；未激活的只读宿主释放不改变持久运行。重启恢复排队叶前再次核验所属图和节点意图，已停止的图不能重新启动排队 Worker。

终态图仍参与报告对账：若图已持久收取结果、叶报告尚未 ack 时崩溃，恢复只补齐确认，不重跑节点，避免反复排入空恢复轮次。

run 状态为 `running/stopping/completed/failed/cancelled/interrupted`。节点状态为 `pending/dispatch_pending/dispatched/running/stopping/completed/failed/blocked/interrupted/cancelled`；未选 switch 分支不生成 Worker，不要求客户端支持一个不存在的 skipped 枚举。等待依赖和排队从持久节点及 AgentRun 读取，客户端不从阶段顺序猜测。

AgentRun 归属区分普通父派工与 workflow 节点；工作流收集叶报告，普通通知和 await_agents 不重复消费它们。父会话每个运行只应接收一次聚合终态，沿用先保存父会话、再 ack 的收据顺序。状态更新使用既有 EventLog 游标与订阅，重新连接先取持久快照再追事件。未知用量保持未知，不补为 0；复用节点保留来源，不算新执行。

## 使用入口

| 入口 | 操作 |
| --- | --- |
| 父 Agent `workflow_read` | `schema/list/get/history/validate`；validate 只校验，不保存 |
| 父 Agent `workflow_control` | `save/run/stop/retry/resume`，经原审批与审计；运行立即返回持久句柄 |
| Runtime `workflow.*` | 与上述 operation 对应，所有请求绑定 `parent_session_id`；详情见 [Runtime API](RUNTIME_CONTROL_API.md) |
| Web | 会话工作流卡片与 Runtime 工作流列表，查看阶段/节点、结构化结果、报告、模型、用量、历史及整组控制 |
| TUI | `/workflow`、`/workflow list`、`/workflow history`、`/workflow get <run-id>`、`/workflow stop <run-id>`、`/workflow retry <run-id>`、`/workflow resume <run-id>` |

Web/TUI 使用同一 run ID 和持久状态，retry/resume 展示新运行。中文、英文、日文文案沿用现有本地化体系。加载、空状态、错误、无控制权限和恢复状态需要通过实际 UI 验收；未知用量显示未知。TUI 创建/运行定义可在对话中请求父 Agent 使用工作流工具，斜杠入口负责观察与整组控制。

对话内 run 直接挂接已经持有执行权的 BuiltHarness driver，不排一个等待本轮释放执行权的控制轮次。外部 Runtime 调用只完成准入并持久排入会话控制轮次，返回后由宿主推进，幂等全局锁内不等待模型或审批。父模型本轮结束、暂时没有活跃 Worker 的阶段间隙均不能让无头等待或目标完成门提前退出。

### foreach 流水线示例

先用 schema 获取当前格式，validate 后 save，再 run 传入 `{"items":["模块 A","模块 B"]}`。每项 inspect 完即可开始自己的 verify，无需等待另一项；summary 显式依赖全部 checks 汇总。以下为完整定义示例，不是本轮已运行的验收记录。

```json
{
  "version": 1,
  "id": "inspect-check-v1",
  "name": "逐项调查与核验",
  "input_schema": {
    "type": "object",
    "properties": {
      "items": {"type": "array", "items": {"type": "string"}, "maxItems": 16}
    },
    "required": ["items"],
    "additionalProperties": false
  },
  "phases": [{"id": "checks", "name": "调查核验"}, {"id": "summary", "name": "汇总"}],
  "nodes": [
    {
      "id": "checks",
      "kind": "foreach",
      "phase": "checks",
      "items": {"$ref": "input#/items"},
      "body": {
        "nodes": [
          {
            "id": "inspect",
            "kind": "agent",
            "profile": "generalist",
            "prompt": "只读调查给定项，返回含证据的 JSON 对象。",
            "input": {"$ref": "item#"},
            "output_schema": {"type": "object"}
          },
          {
            "id": "verify",
            "kind": "agent",
            "depends_on": ["inspect"],
            "profile": "reviewer",
            "prompt": "独立核对输入证据，返回 JSON 对象。",
            "input": {"$ref": "node:inspect#"},
            "output_schema": {"type": "object"}
          }
        ],
        "output": {"$ref": "node:verify#"}
      }
    },
    {
      "id": "summary",
      "kind": "agent",
      "phase": "summary",
      "depends_on": ["checks"],
      "profile": "generalist",
      "prompt": "汇总各项核验结果，返回 JSON 对象并保留证据来源。",
      "input": {"$ref": "node:checks#"},
      "output_schema": {"type": "object"}
    }
  ],
  "output": {"$ref": "node:summary#"}
}
```

## 实现位置与实际验收（2026-11-04）

协议源为 `crates/willdeep-runtime-protocol/src/workflow.rs`；引擎、有限 Schema、引用和存储在 `crates/willdeep-core/src/workflow/`；会话执行在 `crates/willdeep-cli/src/harness/workflows.rs`，外部准入/恢复在 `crates/willdeep-cli/src/daemon/workflows/`。对话定义管理由 `harness/agent_library.rs` 和 `agent_registry/management.rs` 复用本机事务。Web 使用独立 Workflow 组件，TUI 使用 `tui/workflow_ui.rs`。

| 验收对象 | 已运行的证据 |
| --- | --- |
| 协议与纯引擎 | Rust workspace 全量测试通过，覆盖 DAG/引用/Schema、foreach 顺序、分支、循环耗尽、硬限与输出拒绝 |
| 生命周期 | Harness 专项及全量测试通过；覆盖持久意图、接受后关联前崩溃、全量取消、只读宿主释放、未接管新运行保护、终态叶 ack、排队恢复授权和结果复用 |
| 调度与安全 | 默认上限 8 并发/64 排队由原生测试验证；隔离 Runtime 配置 3 并发/2 排队，以 18 个工作流叶和 1 个普通 Worker 实测共享饱和、背压及 19 个唯一完成记录 |
| 专业管理 | CLI/core/宿主测试通过，覆盖创建竞争、安装 CAS、同 request 异参、local/private 遮蔽、账号切换、官方先 fork、已保存未刷新及同轮目录更新 |
| 同轮与旧快照 | 真实对话完成创建→新派工→更新/绑定→新派工；在途旧 Worker 保持原定义。强制终止隔离 daemon 后显式恢复，复用成功节点并保留原专业目录/模型，新普通派工使用新目录 |
| 逐项流水线 | 真实 Runtime 阻塞较慢项的调查，同时观测较快项已完成核验；随后释放慢项并完成汇总，四个调查/核验叶均只调用一次 |
| Runtime API/E2E | `scripts/verify_workflow_runtime.rb` 已运行通过，输出 JSON + Markdown；包含父回复后后台推进、幂等、JSON 拒绝、停止、重试、崩溃恢复、共享调度与聚合确认 |
| Web | 43 个 Vitest 文件、310 个测试通过，lint/build 通过；浏览器使用真实内嵌 Web 与隔离 Runtime，具体交互证据及限制见验收报告 |
| TUI | 原生测试通过；独立 Ruby 进程驱动真实 PTY，在 110×34 和 42×22 下完成 list/get/history、输入编辑、翻页与 Ctrl-C，未提交新模型轮次 |

可重复运行以下专用验收；需要先用 Yarn 构建 Web，再构建当前 Rust 二进制，且 Web 依赖中已有 `@xterm/headless`：

```bash
ruby -EUTF-8 scripts/verify_workflow_runtime.rb
```

默认报告为 `build/verification/workflow-runtime/report.json` 和 `report.md`，终端另附 `tui-report.json`、`tui-report.md`、原始 ANSI、输入操作与渲染屏幕。`--serve` 在本机 19378 端口保留隔离预览，`--skip-tui` 会明确记录跳过，不能视为完整终端验收。`scripts/verify_workflow_tui.rb --fixture <fixture.json>` 可附着仍存在的隔离目录单独验收，不读取日常配置或凭据。

版本核对、fmt、Clippy、源码行数和 npm 包契约均已执行，详细结果见 [验收报告](verification/0.141.0-rc1.md)。Yarn v1 锁文件没有项目自身版本，无需重生成；源码安装清单 `assets` 为空，发行时由流水线下载并校验产物后填充。实现验收结束时尚未提交、推送、打 tag、安装或发布，后续发行状态以 [GitHub Releases](https://github.com/niuwoai/willdeep-cli/releases) 为准；该次本机验收未覆盖 Windows/Linux 原生执行与真实远端模型调用。
