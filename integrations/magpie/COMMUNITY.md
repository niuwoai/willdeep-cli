# Magpie 社区收录草案

状态：本地材料，未发布、未提交、未收录。包版本均为 `0.1.0-rc2`。源仓库为 https://github.com/niuwoai/willdeep-rs ，目录 `integrations/magpie/agent-willdeep` 和 `integrations/magpie/agent-willdeep-cli`；当前工作区内容尚未提交，上述 URL 仅表明项目来源，不声明远端已包含这些文件。

## 条目资料

| 项目 | WillDeep Desktop | WillDeep CLI |
| --- | --- | --- |
| Agent ID | `willdeep` | `willdeep-cli` |
| 名称 | WillDeep | WillDeep CLI |
| 本地包名 | `willdeep-magpie-agent-desktop` | `willdeep-magpie-agent-cli` |
| 图标 | `agent-willdeep/icon.svg` 及相同内嵌 data URL | `agent-willdeep-cli/icon.svg` 及相同内嵌 data URL |
| 预计最低客户端 | `1.427.0-rc1`，需首次启动 | `0.142.0-rc1`，命令 `willdeep` |
| 已验证 Magpie | `0.1.2891`，隔离 HOME 的插件宿主操作 | 同左 |
| 接口文档基线 | Magpie `759b4d1d7da8b983860539e3bd8ce1eaf70a06a4` | 同左 |

现有本地包不使用 `@magpie-community` scope，`private: true`。独立仓库发布后的 registry 条目应省略 `community` 标志。只有与社区维护者约定托管/发布方式后，才准备相应 `@magpie-community/agent-<name>` 包及 `community: true` 条目；此材料不主张已取得任何 scope 权限，也未检查非社区包名是否可在 npm 注册。

`community/registry.entries.draft.json` 只提供条目数组，不冒充上游 registry 的完整顶层结构；使用时按上游最新结构合入。因为包尚未发布，它不是可直接安装的生产市场列表。

## 中文申请正文草稿

拟新增两个独立的 Agent 插件：WillDeep 和 WillDeep CLI。两包通过 Magpie 支持的 `magpie.agent` 模块导出描述符，分别管理自己的 JSON v1 连接文件，不修改用户的原 TOML、Provider 库或钥匙串。

连接只接受本机回环网关和各客户端固定的公开标记；真实网关 Key 和远端共享网关首版拒绝。模型保留完整 ID，未知元数据写为 `null`。连接和模型变化只影响没有显式选择的新会话；断开恢复客户端原默认，已有集成会话续聊需要重连或改选。

Desktop 预计最低版本为 1.427.0-rc1，首次启动后创建自己的支持目录。CLI 预计最低版本为 0.142.0-rc1，可执行文件为 willdeep。自定义 WILLDEEP_HOME 必须使用匹配路径的本地插件副本，不能依赖另一个进程自动继承环境。

插件原生 JS 测试及 Magpie 0.1.2891 的隔离宿主加载、A/B 设模、独立断开、原生模型恢复和自定义根隔离验证均有可复现命令。宿主测试没有发送模型请求；双端完整推理与会话恢复仍需对应客户端 E2E 证据。安装/断开说明和中英文提示随包提供。正式提交时应补上已发布包名、可访问源码 revision、完整消费端验收结果及维护方式。

## English submission draft

Proposed additions: two independent Agent plugins, **WillDeep** and **WillDeep CLI**. Each package exports one descriptor through `magpie.agent` and manages a separate JSON v1 connection file. Existing TOML, Provider preferences and Keychain entries are not edited.

Connections accept only loopback HTTP(S) and a fixed public client marker. Real gateway keys and remote/shared gateways are rejected in the initial version. Complete model IDs are preserved, and unknown model metadata is written as `null`. Connections and model changes apply to new sessions without an explicit choice; existing sessions retain their Provider and model. Disconnect restores the client's original defaults, while explicitly integrated sessions require reconnection or another Provider before continuing.

Expected minimum clients: **WillDeep Desktop 1.427.0-rc1**, launched once to create its own support directory, and **WillDeep CLI 0.142.0-rc1**, executable **willdeep**. A custom WILLDEEP_HOME requires a matching local descriptor; changing the CLI environment does not update a running Magpie.

Native JavaScript tests and isolated real-host checks on **Magpie 0.1.2891** cover module loading, A/B model writing, independent disconnect, native-model restoration and custom-home isolation. The host checks issue no inference requests, so full consumer inference/session tests must be supplied separately. Installation, disconnect and bilingual notices are included in each package. Before submitting, replace local identifiers with authorized published package names, a reachable source revision, full consumer E2E evidence and maintainer details.
