# 0.143.0-rc9 聊天配置验收

日期：2026-11-05。项目阶段：既有项目迭代；基于 `f2094c07`（0.143.0-rc8）补齐默认聊天的压缩配置入口。

本目录记录本地工作树的实现与验证，不代表版本已提交、合并、安装或发布。

## 实现与验收范围

- 默认父聊天通过 `configuration_read` 查看受限 schema、脱敏快照和预览；`configuration_write` 只提交宿主保存的 `plan_id`。首版只编辑 `[agent] compressor_model`、`compressor_profile`、`compressor_context_window`。
- 配置宿主绑定实际加载路径，遵守自定义 `--config` 和 `WILLDEEP_HOME`，将现有符号链接固定到目标；模型不能覆盖路径、编辑内容或确认参数。
- 复用内嵌 Ruby 配置服务的 schema、plan/apply、备份、原子写入和写后校验。目标锁内检查文件 revision；共享备份目录的独立锁覆盖备份命名和写入。
- 预览先经 Rust 完整配置解析、Runtime 业务校验和压缩 Provider 构建。私有候选文档、原始 TOML、注释和凭据均不返回模型；底层解析与传输错误不展开源行。
- 只读禁止写入；strict/smart/workspace-write 需要现有审批，full-access 沿用已有授权；Worker、委派子会话及受限 MCP 任务不获得入口。不确定结果的配置写重放先验证计划及权限，再进入一次性审批。
- 下一次新 Runtime Turn 读取新配置，当前 Turn 保留快照。TUI 空闲时 `/compress` 重读配置，取得会话执行权后整体刷新候选链与窗口；一次压缩贯穿分块、合并和回退使用同一快照。

## 隔离边界

Ruby 服务验收使用合成配置和临时目录。真实 Runtime 验收通过清空继承环境、临时 `HOME`/`USERPROFILE`/`WILLDEEP_HOME`/XDG 路径及动态回环 Provider 完成；仅启动和停止夹具自己的 Daemon。

未读取个人配置或凭据，未连接个人 9847 Daemon，未访问或干扰 19382 预览，未安装或自动批准用户插件。本次没有浏览器界面改动，不启动现有 Web 服务；Web 只执行构建以提供 RustEmbed 所需资产。

## 验证结果

| 检查 | 实际结果 |
|---|---|
| Ruby 配置服务压缩语义验收 | 19/19 通过，见 [compressor.md](compressor.md) / [compressor.json](compressor.json) |
| 既有 `config_plugin_test.rb` | 通过 |
| Rust 配置宿主与 TUI/Core 定向回归 | 26/26 通过：配置宿主 6、TUI dispatch 3、Core host tools 12、uncertain replay 5 |
| 真实 CLI → 隔离 Runtime → 回环 Provider | 4/4 通过：内置工具保存、敏感信息隔离、普通下一轮使用新压缩模型、strict 允许预览但拒绝保存；见 [runtime.md](runtime.md) / [runtime.json](runtime.json) |
| 全工作区严格 Clippy 与最终 CLI 构建 | `cargo clippy --workspace --all-targets --offline --locked -- -D warnings` 与最终 `cargo build` 均通过 |
| Web 生产构建 | `yarn --cwd web build` 在 rc9 元数据下通过；Vite 的大包体积提示未阻止构建 |
| 版本一致性 | `ruby scripts/check_release_version.rb 0.143.0-rc9` 通过；最终二进制 `--version` 返回 `willdeep 0.143.0-rc9` |
| 格式与空白 | `cargo fmt --all -- --check`、`git diff --check` 通过 |
| 源码规模 | `ruby -EUTF-8 scripts/check_source_size.rb` 通过，无超过 3000 行文件 |

最终统一流水线退出码为 0，按顺序完成 Web 生产构建、上述 26 项 Rust 定向回归、全工作区严格 Clippy、CLI 构建及 4 项真实 CLI/Runtime 验收。此前已通过的相关定向验证另包括 Core 上下文压缩 42 项、CLI compressor 9 项及 CLI compression 11 项；它们与最终重验的 26 项有部分范围重叠，不合并计算用例总数。

Ruby 压缩服务的 19 项覆盖三字段预览与保存、确认拒绝、精确备份、窗口类型/范围、档案引用、同批依赖修改、相同 revision 并发单胜者、不同目录同名目标共享备份目录、C locale 中文路径，以及 validate 的依赖/范围错误。

Rust 定向回归覆盖自定义路径与符号链接、旧计划/跨宿主计划、参数与范围拒绝、完整 Runtime 语义验证、敏感信息投影、五档权限、拒绝审计、共享切档、委派边界、不确定结果重放、TUI 刷新失败后继续交互，以及压缩候选/窗口快照一致性。

## 可重复执行的命令

在此工作树根目录执行；Runtime 脚本使用本工作树构建的二进制，不调用全局安装的 `willdeep`。

```bash
ruby scripts/test/config_compressor_test.rb docs/verification/0.143.0-rc9/chat-config
```

```bash
ruby scripts/test/config_plugin_test.rb
```

```bash
yarn --cwd web build
```

```bash
cargo test -p willdeep --bin willdeep configuration_host --offline --locked
```

```bash
cargo test -p willdeep --bin willdeep tui::dispatch::tests --offline --locked
```

```bash
cargo test -p willdeep-core --lib tools::host_tools::tests --offline --locked
```

```bash
cargo test -p willdeep-core --lib agent::uncertain::tests --offline --locked
```

```bash
cargo clippy --workspace --all-targets --offline --locked -- -D warnings
```

```bash
cargo build -p willdeep --bin willdeep --offline --locked
```

```bash
ruby scripts/test/config_chat_runtime.rb target/debug/willdeep docs/verification/0.143.0-rc9/chat-config
```

```bash
ruby scripts/check_release_version.rb 0.143.0-rc9
```

```bash
ruby -EUTF-8 scripts/check_source_size.rb
```

```bash
cargo fmt --all -- --check
```

```bash
git diff --check
```

## 过程中的失败与修正

- 初始工作树没有 `web/dist`，RustEmbed 因缺少资产编译失败；使用 yarn 离线缓存安装依赖并真实构建，没有创建占位资产。
- 中文路径在 C locale 下被 Ruby 作为 ASCII-8BIT 处理；私有服务的三个路径环境变量显式使用 UTF-8，新增真实 stdio 回归后通过。
- 增加压缩 Provider 构建校验后，原测试档案缺少独立端点与合成凭据；夹具补齐后不依赖进程外部 Provider 环境。
- 多行 TOML 测试曾错误期待编辑器接受三引号字符串；现按编辑器既有能力检查安全拒绝、无可提交计划，再验证单行合成凭据不泄漏。
- HTTP 夹具初次没有处理模型探测 GET，随后补齐 models 响应。
- 权限拒绝 E2E 曾把 `read-only` 写入 TOML，而现有文件枚举仅接受 strict/smart/workspace-write/full-access；夹具改为 strict 非交互拒绝。Core 另行验证 ReadOnly 硬边界。
- 共享备份目录锁引入后，宿主测试把 `.backup.lock` 当作备份，出现 `left: 2, right: 1`（5 项通过、1 项失败）；断言改为只统计 `.bak`，继续核对备份原文字节。
- 格式检查发现一处 Rust 导出顺序差异，已按 rustfmt 修正。源码规模脚本在 C locale 下直接运行因 US-ASCII 编码失败，改为 `ruby -EUTF-8` 后通过。
- 独立复核确认并修复三处边界：共享目录同名备份覆盖、`/compress` 异常源行泄漏、失效计划先弹不确定结果重放审批；分别补充锁、脱敏错误和审批前校验回归。

## 限制与未覆盖环境

- 现有 Ruby 配置编辑器是受限 TOML 解析器，不支持三引号多行字符串等完整 TOML 语法；此类配置安全拒绝预览。可先手动改用受支持的单行字符串或环境变量引用。
- 保存前检查配置语义和 Provider 构建，不调用模型端点；网络可达性、服务端凭据接受情况和真实摘要质量不在该检查范围。
- `/local` 复用启动 Agent；只有 `/compress` 成功构建设置并取得执行权后，后续本地压缩才复用刷新设置，其余配置仍为启动快照。摘要请求之后失败或取消不会回退已刷新设置。
- 每个宿主仅保留最新成功计划，新 Runtime Turn 需要重新预览；回执丢失先查看实际状态，再重新预览，不自动重放未知写入。
- 文件锁仅协调遵守配置服务锁协议的写入，普通外部编辑器不参与该协议。
- 本机验证环境为 macOS。没有执行 Windows/Linux 原生测试、完整工作区测试、真实远端模型请求或真人 PTY 交互验收；TUI 路径由 dispatch/Core 自动化测试验证。
- 所有变更仅在本工作树；未提交、推送、打 tag、创建 PR、安装或发布。
