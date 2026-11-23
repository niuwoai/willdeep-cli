# MCP 启动诊断测试的 CI 环境隔离

日期：2026-11-23。候选版本：`0.154.0-rc9`。本轮只调整测试夹具及版本元数据，生产运行逻辑、协议和数据格式不变。

## 失败证据

PR 172 的 `b6e8c1a0783edcf24df99673e11243006cd39901` 在自托管 Linux worker 中失败，[运行日志](https://github.com/niuwoai/willdeep-cli/actions/runs/45048568038/job/137288447659) 指向：

`mcp::startup_tests::missing_process_reports_server_and_stage_without_configuration_secrets`

该轮 core 单元测试为 1384 通过、1 失败、7 忽略。失败断言要求原始错误为 `McpError::Io(NotFound)`；前面的服务名、stdio、process spawn 和合成敏感参数不泄露断言已通过。日志没有打印实际 I/O 错误种类，因此不能仅凭日志断言该 runner 必然返回了 PermissionDenied。

同一提交的父 CI 通过完整托管 suite 兜底成功；独立 worker job 的红色检查仍保留。自托管 workflow 的 `continue-on-error` 意味着 workflow 总体 success 不能替代具体 worker job 与测试日志结论。

## 修复

原测试使用随机裸命令名，由操作系统搜索 runner 的真实 PATH。Unix 搜索不可访问的 PATH 目录时可能返回 PermissionDenied，故随机名称不能保证 NotFound。

改为创建唯一的私有临时目录，直接指定该目录下不存在的绝对可执行路径。真实 MCP 连接完成后删除空夹具目录；错误类型、错误链、服务名、启动阶段及合成敏感参数不泄露断言保持严格，不增加 PermissionDenied 白名单，不修改运行时启动逻辑。

## 验证

本地针对性验证通过：MCP 单元测试 28 通过、0 失败、0 忽略；内部版本钉测试 1 通过；core（含测试目标）严格 Clippy、rustfmt、版本元数据及 git diff --check 通过。未重跑无关的整套 TUI 测试或浏览器预览，完整 Linux/多平台结果由推送后的 CI 确认。结果摘要同时保存为 [JSON](mcp-startup-ci.json)。

另以独立进程设置不可访问的 PATH 目录，不修改当前进程环境，再运行修复后的真实 core 原生测试：1 通过。macOS 对裸命令仍返回 NotFound，因此该实验只证明新夹具在该环境下通过，不能声称已在本机复现远端的具体错误类型。

验证辅助程序的首次运行错误地把所有 Unix 平台的对照结果限定为 PermissionDenied，实际输出为 `left: NotFound, right: PermissionDenied`，因而失败；已改为记录本机实际对照结果，并继续严格执行产品测试原有的 NotFound 断言，第二次验证通过。此调整仅涉及本地辅助程序，未放宽 MCP 测试断言。本地辅助源码为 `.build/mcp-path-regression.rs`，未作为生产代码提交。

针对性验证命令：

```bash
cargo test -p willdeep-core --lib mcp:: --locked --offline --target-dir .build/cargo-target --no-fail-fast
```

```bash
cargo test -p willdeep --test workspace_versions --locked --offline --target-dir .build/cargo-target
```

```bash
cargo clippy -p willdeep-core --lib --tests --locked --offline --target-dir .build/cargo-target -- -D warnings
```

```bash
ruby scripts/check_release_version.rb 0.154.0-rc9
```

本机安装仍为已验收的 `0.154.0-rc8`；此次测试夹具修复不替换已安装二进制，不将 rc8 的二进制校验值或 PTY 结果标为 rc9。未公开发行、未创建发布 tag。
