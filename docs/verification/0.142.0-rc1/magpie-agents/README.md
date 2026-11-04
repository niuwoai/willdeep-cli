# Magpie 社区 Agent 插件验收

> 日期：2026-11-04 | 状态：**本地实现与组合验收完成；未发布、未社区收录**
> CLI：`0.142.0-rc1`，最终二进制 SHA-256 `6eb920c5bbdd7580fd7b1e40a5b82a1e989401e444083fb20c28c2f6859e1f0c`
> Desktop：`1.427.0-rc1`，Build `941`
> 插件：`willdeep` / `willdeep-cli` 均为 `0.1.0-rc2`
> 实施基线：CLI `b83372be` / `0.141.0-rc3`；Desktop `4cd9e147` / `1.426.0-rc1`

本轮实现由[需求契约](../../../MAGPIE_AGENT_REQUIREMENTS.md)、[跨端设计](../../../MAGPIE_AGENT_DESIGN.md)及[执行台账](../../../MAGPIE_AGENT_PLAN.md)定义。所有验收使用临时 HOME、独立 `WILLDEEP_HOME`、隔离配置和确定性本机模拟上游；不读取真实供应商账号或钥匙串，不发起付费推理，不安装、升级或重启用户程序。测试统一设置 `WILLDEEP_LOCAL_DISCOVERY=0`，避免探测用户本机网关。

## 结果汇总

| 范围 | 结果 | 证据 |
| --- | --- | --- |
| Rust workspace 测试 | 2320 项通过、11 项忽略、0 失败 | `cargo test --workspace --all-targets` |
| 严格 Clippy / 格式 | 通过 | `cargo clippy --workspace --all-targets -- -D warnings`、`cargo fmt --all -- --check` |
| 源码规模 | 通过 | `ruby -EUTF-8 scripts/check_source_size.rb`：1320 个文件，0 超限；`main.rs` 2981 行 |
| Windows 交叉编译 | 通过 | `cargo check` / `cargo clippy -D warnings --target x86_64-pc-windows-gnu`；**仅为交叉编译，非原生运行** |
| CLI 隔离组合链 | 21/21 通过 | `build/verification/magpie-agents-0.142.0-rc1-final/` |
| CLI + 真实 Magpie 链 | 22/22 通过 | `build/verification/magpie-agents-real-0.142.0-rc1-final/`；Magpie `0.1.2891` |
| 真实 PTY | 通过 | 同一报告内 `pty.json`：`@xterm/headless` 渲染 110×34 与 42×22 |
| 插件原生测试 | 19/19 通过 | `yarn --cwd integrations/magpie test` |
| 插件归档与宿主 | 两包各 2/2；宿主 12/12 | `yarn --cwd integrations/magpie test:packages`；模型请求 0 |
| Web 页面（最终二进制） | 通过 | `build/verification/browser-0.142.0-rc1-final/`：DOM、截图、console 报告 |
| Desktop 原生测试 | 19/19 通过 | Xedit `build/magpie-native-final/report.json` |
| Desktop 契约测试 | 14 项通过、27 个 fixture | Xedit `build/magpie-integration-tests/report.json` |
| Desktop 构建 | `BUILD SUCCEEDED`，Debug arm64，无签名 | Xedit `build/local/build_report.json` |

所有 CLI 组合验收都在最终二进制上重跑；此前中间构建的哈希已作废，不作为本报告证据。

## CLI 与 Runtime 组合验收

入口：

```bash
ruby -EUTF-8 scripts/test/magpie_agents_runtime.rb --binary target/debug/willdeep --report-dir build/verification/magpie-agents-0.142.0-rc1-final
```

真实 Magpie 链在相同命令上追加 `--magpie-binary /Applications/magpie.app/Contents/MacOS/magpie`，并设置 `MAGPIE_BUN` 指向既有 Bun。覆盖：

- Runtime 先启动、后写入连接；不依赖重启 daemon 或注入环境。
- 未连接使用原生默认；连接后新会话使用 A；外部 A→B 后旧会话保持 A、新会话使用 B。
- 旧无固定绑定会话恢复跳过集成默认；客户端新建会话固定 `magpie-cli`。
- 显式原生 Profile 优先；`--config`、显式模型跳过隐式集成。
- `enabled:false` 与宿主恢复后仅含 `model` 的对象均恢复原生默认，不应用该模型。
- 固定引用集成的会话断开后明确报错，不转发到其他供应商或同名用户 Profile。
- 无关 `invalid` 不阻断显式原生选择，但阻止隐式新会话。
- 自定义 `WILLDEEP_HOME` 不回退标准 Home；同名用户 `magpie-cli` Profile 与真集成来源隔离。
- 拒绝重定向、忽略代理环境；测试前后 TOML 字节不变。
- 真实 PTY 宽 110×34 与窄 42×22 分别产生新模型请求与 `magpie-agent-ok` 回复。

付费请求：0；真实凭据：0。

## Web 验收

最终二进制以隔离夹具启动，`127.0.0.1:19847`。使用无头 Chrome（`--headless=new`）与 CDP 驱动真实页面：

- `GET /api/models/default` → `magpie-cli` / `mock/model-a`；`GET /api/settings/model-routing` 的 `providers` 同时含 `native` 与 `magpie-cli`。
- 打开模型选择器后，DOM 中 Provider 下拉真实可见，选项为 `native · openai-compatible · 默认` 与 `magpie-cli · openai-compatible`，当前选中 `magpie-cli`；页面标题显示 `选择模型 · mock/model-a`。
- 在真实输入框输入「验证 Magpie 集成，请返回测试响应。」并点击发送，页面收到并显示 `magpie-agent-ok`，页脚版本为 `v0.142.0-rc1`。
- 新会话详情固定 `provider_id: magpie-cli`、`model: mock/model-a`。
- 刷新页面并监听 `Runtime` / `Log` / `Network`：无 console 错误、无未捕获异常、无失败请求。
- 测试 TOML 中不存在 `magpie-cli`，临时 Provider 未被持久化。

产物：`dom.html`、`page.png`、`picker-open.png`、`chat-reply.png`、`console-report.json` 与三个 CDP 脚本，位于 `build/verification/browser-0.142.0-rc1-final/`。

## 插件与真实宿主

- 两个包可分别从本地目录安装；一个包只导出一个 Agent，`package.json` 的 `magpie.agent` 为唯一入口，无安装脚本、无 Node API 依赖。
- 包内 `contract.js` 在两个包间字节一致；图标、消息与测试各自独立。
- 真实 Magpie `0.1.2891`（`759b4d1d…` 契约基线）宿主验证：双包注册、互相不误检测、A/B 设模互不影响、独立断开、原生模型恢复、自定义 CLI Home 隔离；全程 0 次模型请求。
- 归档：`agent-willdeep-0.1.0-rc2.tgz`（SHA-256 `d399bcd6…362666`）、`agent-willdeep-cli-0.1.0-rc2.tgz`（SHA-256 `ae6354c9…616069`）。两包 `private: true`，未发布。

## Desktop

Xedit worktree 内完成的验证：

- 独立 Swift 契约测试 14 项，覆盖 27 个共享 fixture 原始输入，输入 SHA-256 前后不变。
- Xcode 原生宿主测试 19/19（2 个 suite），使用临时 Home、测试 Provider 库与 XCTest 内存钥匙串；真实 HTTP 只连接随机本机端口的 `MagpieFakeUpstream`。
- 隔离 Debug 构建成功，版本 `1.427.0-rc1`、Build `941`，未签名、未公证、未安装。

## 未覆盖与限制

- Linux/Windows 只做了 Windows GNU 目标交叉 check/Clippy，未在 Windows 或 Linux 原生运行；macOS 为唯一原生验收平台。
- Desktop 未运行全仓完整测试集，未进行手动 App 前台点击真实 Magpie 的组合验收；其结论来自上述原生测试、契约测试与构建。
- Web 验收使用无头 Chrome + CDP，未使用应用内 Browser 面板：面板启动器固定读取原仓 `.claude/launch.json`，不识别 worktree 配置；9847 端口为用户真实 daemon，未被使用。
- 本轮两次多角度独立代码审查均因服务端连接中断未产出结论；最终结论基于可复现的测试与差异核对，不声称完成过独立审查。
- 没有公开发布、社区收录、提交、合并、推送或 tag；没有修改真实用户配置、钥匙串或正在使用的 daemon。
