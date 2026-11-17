# P0：版本化编辑、多文件协议与工作区 LSP

> 决策日期：2026-11-17。目标开发版本：0.150.0-rc1。既有项目迭代；范围来自 [Pi / OMP 差距分析](../research/2026-11-17-willdeep-cli-gap-analysis.md) 的 P0。
> 本文件记录实施契约；实际验证结果见 [0.150.0-rc2 P0 验收](../verification/0.150.0-rc2/p0-coding-tools.md)，未执行的检查不得视为通过。P0 功能在 rc1 引入，当前工作区整合版本为 rc2。

## 产品范围

复用 Rust Runtime 的 `ToolRegistry`、审批、Hooks、工具目录、取消、Worker 写集和工作区路径检查。CLI、TUI、Web 的模型工具调用共用这些入口，不新增客户端私有执行通道。

本轮实现三个互相配合的能力：编辑前检查文件版本、可审阅的多文件创建/替换、Rust 与 TypeScript 的 LSP 查询，并补充固定任务的离线结果基线。语义重命名、AST 改写、DAP、文件删除/重命名和真实模型付费评测不在本轮范围。

## 文件版本

- `read_file` 读取 canonical 工作区路径的实际完整 UTF-8 内容，计算 `sha256:<64 位十六进制>`。首行返回 `[version: sha256:…]`，正文保持原有 1-based 行号与分页格式；`max_bytes` 限制带行号的正文，版本行和续读提示另计。版本属于整文件，即使只返回一页。
- Registry 保存实际读取基线。`edit_file.expected_version` 可显式指定版本；省略时使用该 Registry 的实际读取基线。两者均缺失则拒绝，提示先读取。跨进程恢复后的 Registry 不凭聊天文本自动恢复读取权限。
- 版本匹配后仍执行原有精确文本匹配、唯一性和 `replace_all` 校验。冲突需要重新读取和判断改动；不得靠猜测新哈希绕过检查。成功返回并登记新版本。
- 所有本进程 Registry 的 `create_file`、版本编辑及多文件应用共用写入协调锁。审批等待不占锁；写入前复核路径和版本。
- SHA-256 内容版本不能识别内容相同的 ABA 更改，普通文件系统也不提供对任意外部进程的通用 compare-and-swap。本机制约束合作中的宿主写入并在提交前检查外部变化，不承诺任意外部写入竞态下的强事务隔离。

## 多文件变更协议

工具：`file_changes`，操作仅有 `preview` 和 `apply`。

预览提交 `changes` 数组，每项包含 `path`、`operation`（`create` / `replace`）、完整 `content`，替换可带 `expected_version`。`replace` 必须有显式版本或实际读取基线，且旧内容必须是有效 UTF-8 文本，二进制内容在生成差异前拒绝；`create` 要求目标不存在且父目录已经存在。预览不创建目录、不写目标文件，返回有界差异和不可猜测的 `preview_id`，绑定真实目标、原版本和新内容。

应用只接受 `preview_id`，不能修改其内容。预览有数量、内容量和寿命限制，使用一次即失效；重启不保留。应用在审批前及审批后复核整个写集、真实目标、工作区边界、Worker 允许路径和原版本；预检失败不写任何目标。审批面展示绑定的变更。

逐文件安装使用同目录临时文件与原子安装。多文件不宣称文件系统原子事务。中途失败时尝试恢复已安装文件，恢复前检查是否仍为本次写入内容，不能覆盖外部后续修改。回执区分 `applied`、`rolled_back`、`conflicts`、`recovery_failed` 和 `cleanup_failed`；`files_touched` 仅列实际落盘且未恢复的目标，以绑定的工作区相对真实路径报告，避免符号链接改指后回执指向另一文件；`applied` / `rolled_back` 保留请求路径用于核对历史。预览包含 `target_path`；非 UTF-8 工作区根目录的 `canonical_path` 为 null、`canonical_path_display` 只作展示，相对目标路径仍须有效 UTF-8。Worker 的事实报告消费该回执，预览和完整恢复不记作成功改动；失败但遗留写入也必须记录。

`file_changes` 按写工具限制：无批准写集的 Worker 不获得该工具，只读会话和只读子会话拒绝调用。未知副作用恢复把其缺失结果当作不确定写入，不自动当成功或安全重试。预览的协议测试与最终文件结果测试分别验收。提交与恢复阶段没有 await，取消不会把这两个阶段截成半套；但提交后的 PostTool hook 等待、结果投递或进程退出仍可能丢失调用回执。缺回执沿用现有 Agent 历史中的未知副作用恢复规则：工具名与规范化 JSON 参数相同的调用重试必须显式一次性审批。换参数后的语义等价操作不在这一匹配范围，直接调用 ToolRegistry 也不持有历史保护；不能宣称取消必定未写入或保证结果已经交付。

## LSP 查询与依赖

工具：`lsp`。首版操作为 `status`、`diagnostics`、`definition`、`references`、`symbols`。通过 `path` 选择 Rust 或 TypeScript/TSX；定位参数 `line` / `character` 为 **0-based UTF-16**，与文件读取的 1-based 显示行号不同。

固定依赖为 `rust-analyzer` 与 `typescript-language-server --stdio`。状态查询不启动服务，缺少依赖明确报告，不自动下载，不接收模型提供的任意服务器命令。查询返回定位、内容快照或版本、截断等信息；未收到诊断不能报告零错误。

服务按 Registry 工作区及语言复用，重用时复核当前执行围栏；审批策略变化时不得复用旧的更宽能力。启动走 `gate_command` 与 `effective_sandbox`，普通只读档不运行语言服务进程。Worker 仅在现有命令策略允许时可启动；不为查询工具绕过窄工种的命令白名单。

Rust 禁止自动 build scripts、proc macros、check / check-on-save；TypeScript 禁止自动类型获取与初始化插件，并保守拒绝 tsconfig/jsconfig 的配置继承、非空 plugins 和 project references。检查包括父目录及嵌套配置，拒绝符号链接，工具链只从工作区外的可信安装解析。这是首版兼容限制。TypeScript 固定 `tsserver.useSyntaxServer=never`，让首次定义与引用查询等待语义服务，避免项目加载期间语法服务仅返回本地导入别名。语言服务自身仍是外部代码进程，查询 API 不意味着操作系统级只读；现有写入围栏也不构成宿主文件读取隔离。

JSON-RPC 使用有界 Content-Length 帧、请求与输出大小、stderr 和超时；处理服务器主动请求，拒绝 `workspace/applyEdit`。取消/超时后销毁或重建有歧义的客户端，不能把旧响应分配给新请求。Registry 释放时清理进程树及后台任务，Unix 使用进程组，Windows 复用 Job Object。

## 固定任务与验收

沿用 `model_eval` 的任务 schema、受保护文件、独立 verifier 和 JSON / Markdown 报告，不建立另一套评测引擎。

新增 `p0-` 任务覆盖 Rust 跨文件引用、TypeScript 同名符号与字符串键引用边界、受保护文件及多文件联动。夹具必须先红、参考答案绿；部分修改、硬编码和改测试不能冒充通过。Rust/Node 依赖和版本要求在任务元数据与说明中明确。

- 原生 Rust 测试：陈旧版本、缺读取基线、审批期间变更、跨 Registry 协调、路径越界、多文件完整预检、回滚冲突、一次性预览、Worker 写集及未知副作用。
- LSP 离线测试：fake server / duplex 传输、主动请求、帧上限、取消、超时、进程生命周期、路径与定位过滤。真实服务器未安装或未运行时明确标注。
- Ruby 离线测试：新任务红绿与反作弊、现有任务完整性、JSON / Markdown 报告输出。
- 发布元数据：Cargo workspace / 内部依赖版本钉 / lock、Web、npm、安装清单、产品概览、架构和 CHANGELOG 一致；仅开发验证，不等于提交、发布或本机安装。

验收记录必须区分工具协议单元测试、固定任务结果测试、真实语言服务试跑和真实模型成绩。任何一类通过都不能代替其他类别。
