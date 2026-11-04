# Magpie 本地 Agent 插件

两个独立包，各 **0.1.0-rc2**，分别提供一个 Agent。当前交付为本地可安装源码；尚未 npm 发布、提交社区或上架。

| 本地目录 | Agent | 检测依据 | 集成文件 | 预计最低客户端版本 |
| --- | --- | --- | --- | --- |
| `agent-willdeep` | `willdeep` / WillDeep | App 创建的 Desktop 私有支持目录 | `~/.willdeep/integrations/magpie/desktop/connection.json` | Desktop `1.427.0-rc1` |
| `agent-willdeep-cli` | `willdeep-cli` / WillDeep CLI | `willdeep` 命令或本 Agent 自己的配置文件 | `~/.willdeep/integrations/magpie/cli/connection.json` | CLI `0.142.0-rc1` |

安装和断开详见各包 README。源码随 [WillDeep CLI](https://github.com/niuwoai/willdeep-rs) 交付；共享契约为 [需求](../../docs/MAGPIE_AGENT_REQUIREMENTS.md)、[Schema](../../docs/schemas/magpie-agent.v1.schema.json) 和 [原始 fixture](../../docs/fixtures/magpie-agent-v1/cases.json)。本目录验证覆盖插件与真实 Magpie 宿主（JS 19/19、归档内原生测试各 2/2、宿主 12/12）；双端消费端与组合验收结果见 [验收报告](../../docs/verification/0.142.0-rc1/magpie-agents/README.md)。

## 运行和文件清单

生产入口是各自 `package.json` 中的 `magpie.agent`。包内 `contract.js`、`messages.js`、`icon.js` 均通过相对路径导入，不跨包依赖；每包可单独复制、安装。无需下载依赖或构建。`package.json` 没有 `main`、`exports` 或安装后脚本，`private: true` 阻止误发布。

- `*.agent.js`：唯一 Agent 的描述符和 `connect`。
- `contract.js`：纯 JS 回环 URL、公开标记、UTF-8 模型和可空元数据验证；两个包保持字节一致。
- `messages.js`：独立中英文资源。当前 Magpie 仅支持一个 notice 字符串，故显示双语。
- `icon.js` / `icon.svg`：内嵌 data URL 和可审查原图，不发外部图标请求。
- `test/agent.test.js`：各包原生 JS 测试；README、CHANGELOG 随包交付。

`connect` 返回七个非模型契约字段。Magpie 自己通过 `model: "model"`、空 `prefix` 写完整 `model.id`。插件严格拒绝真实 Key、远端/非规范 URL、无效模型；窗口合法范围为 `4000..1048576`，输出为 `1..1048576`，未知/非法值写 `null`，输出大于有效窗口时将输出写 `null`。

生产代码只声明数据并做校验，不调用 Node、文件、网络、exec、钥匙串或客户端设置 API。原 TOML/Provider 库不作为插件配置目标。自定义 CLI Home 的完整说明位于 CLI 包 README，不能认为设置 CLI 环境会自动改变 Magpie 进程或发行描述符。

## 原生 JS 测试

在本目录执行：

```sh
yarn test
yarn --cwd agent-willdeep test
yarn --cwd agent-willdeep-cli test
```

无需 `yarn install`。完整测试覆盖两个描述符、真实图标字节、严格回环语法、逐字公开标记、错误脱敏、模型控制字符/UTF-8 边界、元数据上下限、跨客户端标记、包内导入边界和共享 connected fixture 的字段投影。重复键、有界读取和 disconnected/invalid 状态解析属于 Rust/Swift 文件消费端测试；插件不自行读取 JSON 文件。

## 真实 Magpie 宿主验证

本机已验证宿主 **Magpie 0.1.2891**。接口依据固定 [Magpie 759b4d1d 文档](https://github.com/yetone/magpie/blob/759b4d1d7da8b983860539e3bd8ce1eaf70a06a4/site/public/docs/plugins.html)。`go version -m` 读取本次已装二进制的构建 revision 为 `e17b6cefc7b25c19d1e522f9e8a9626bf68912d9`（`vcs.modified=false`），moejs 为 `v0.1.0-alpha.11`；宿主报告记录实际二进制 SHA-256。本机源码 checkout 为 `de671719a0ba04de83103af45298c391d599ceb6`，不含固定基线对象，不能把该 checkout 当作已测二进制源码或声称从 `759b4d1d` 编译测试。

提供已有 Magpie 和 Bun 可执行文件的绝对路径，再运行 Ruby E2E 驱动；以下路径均需替换为本机实际路径：

```sh
MAGPIE_BIN=/path/to/magpie MAGPIE_BUN=/path/to/bun yarn test:host
MAGPIE_BIN=/path/to/magpie MAGPIE_BUN=/path/to/bun yarn test:host --markdown
```

默认输出 JSON，`--markdown` 输出同一组验证场景的 Markdown。也可直接执行 `ruby tools/check-host.rb`。驱动只使用新建的临时 HOME、USERPROFILE、XDG 目录和空 PATH；它关闭账号发现，所有已知网关/市场/代理目标均为临时回环 fake upstream，不启动 daemon 或 GUI、不安装 Bun、不接触真实 Magpie 插件或用户配置。当前宿主的本地 `plugin add` 仍会运行既有 Bun，缺少 `MAGPIE_BUN` 时使用失败运行器阻止自动下载安装。临时目录在退出时清理。

真实宿主共 12 个场景，包括：双包安装和 Agent 注册、仅共享根目录不误检测任一客户端、CLI PATH 与 Desktop 私有目录独立检测、moejs 执行 connect、完整 A/B 模型写入、另一端文件不变、断开清空/恢复原模型、model-only 原生恢复、自定义 CLI 根与标准 Home 隔离。CLI PATH 正向检测使用临时目录中指向 `/usr/bin/true` 的合成可执行文件，只证明宿主的发现行为，不执行或安装 WillDeep。无已安装 Agent 时，明确断言 `magpie ls` 的退出码 1 和错误分类。驱动按 UTF-8 读取宿主输出，可在 Ruby ASCII locale 下执行。没有发出模型推理请求，因此该检查不证明 CLI Runtime/Desktop 的实际会话调用行为。

## 本地打包

直接安装包目录即可。需要本地归档时：

```sh
yarn --cwd agent-willdeep pack --filename ../agent-willdeep-0.1.0-rc2.tgz
yarn --cwd agent-willdeep-cli pack --filename ../agent-willdeep-cli-0.1.0-rc2.tgz
```

这只生成本地 tarball，不发布。Magpie 的稳定验收入口是目录安装；若使用归档，请解压到新目录后安装该包目录。不要只搬 `*.agent.js`，它还依赖本包三个模块。

推荐用完整归档校验入口重新生成这两个交付包：

```sh
MAGPIE_BIN=/path/to/magpie MAGPIE_BUN=/path/to/bun yarn test:packages
MAGPIE_BIN=/path/to/magpie MAGPIE_BUN=/path/to/bun yarn test:packages --markdown
```

`tools/check-packages.rb` 为 Ruby 驱动，默认输出 JSON，`--markdown` 输出 Markdown。它在临时 HOME/XDG 中调用已安装的 Yarn，不安装依赖；逐项核对归档仅有 `package.json` 和 `files` 声明的 8 个文件（每包共 9 个），并与源码逐字节比较。每包解压后独立运行 2 项原生测试，再将两个解包目录交给同一真实宿主的 12 场景校验。全部通过后才将 tarball 复制到本目录，报告包含版本、完整文件清单和 SHA-256。可用 `test:host --packages-root /绝对路径` 单独验证包含两个包目录的其他本地根。旧版归档可能仍在本地，交付以本次成功报告的版本与摘要为准。

## 社区材料与验收结论

[COMMUNITY.md](COMMUNITY.md) 提供中英文收录草稿，`community/registry.entries.draft.json` 提供条目片段。当前包名未核验 npm 可用性，保持私有；没有声称拥有社区 namespace 发布权。公开发布、迁入社区 scope、修改 registry 或外发申请都需要单独授权。

本地组合验收已完成：CLI/Runtime 隔离链 21/21、真实 Magpie 链 22/22（CLI 最终二进制 `6eb920c5…9e1f0c`），Desktop 原生测试 19/19，本目录的宿主校验 12/12。完整结果、测试环境与未覆盖项见 [验收报告](../../docs/verification/0.142.0-rc1/magpie-agents/README.md)；本目录的宿主检查只覆盖插件与 Magpie 宿主，不能单独代表双端会话与推理语义。
