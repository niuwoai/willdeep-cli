# P0—P3 智能体库验收

日期：2026-10-14。Rust `0.101.0-rc1`、Mac `1.419.0-rc1 / build 916`、some.im `0.248.0-rc2`。本文件记录可观察的实现与验证结果；RSI 候选、评测和晋升属于下一阶段。

## 实际跨端往返

服务使用本机独立 Docker MySQL 8.0.36，数据库 `codex_agent_registry_test`、端口 `33316`，HTTP 仅监听 `127.0.0.1:33317`。账号、用户、租户、会话和签名密钥全部是合成测试数据。测试不连接生产数据库。

Mac 验收程序直接编译应用的 `AgentDefinitionPackage.swift` 与 `AgentRegistryService.swift`，没有重写 HTTP 客户端；Rust 使用真实 `target/debug/willdeep agents` 二进制；Web 使用实际 React 页面和真实服务路由。

| 场景 | 观察结果 |
|---|---|
| Mac 创建 → Rust 同步 | `user/mac-cross-client` 的 rc1 包下载成功，hash `0824f301bcd37588454e61fcad7cb2903a1c391b2861d9c45a11b03cec34c5de` |
| Rust 导入 rc2 → 同步 → Mac 读回 | 两端 version 为 `1.0.0-rc2`，hash 一致：`d30e14565386bee1dee5eb21bb2b1e6eba786b0224077de8efef2f5815bd0512` |
| Mac 新版与旧 ETag 写入 | rc3 成功；陈旧更新被拒绝，head 保留 `a54ae66aedb679e733ad39f3c5cbaa5d1675c66458ce7eb2fbf1c86a2caa12b2`；Rust 下一次同步下载该版本 |
| Mac 删除 → Rust 同步 | 服务产生 tombstone，Rust 的同步报告包含 deleted，后续可用目录不再加载该定义；历史包保留 |
| 第二账号 | Mac 列表不含第一账号定义，Rust 同步 downloaded/uploaded/deleted 均为空 |
| 固定官方包 | 两端下载并验签 `official/code-reviewer`，相同 hash `a63f949f0e5f1488b50f16d9da6ee88cee0653c4dbbc091a36a7c5743ea64540` |
| Web 新建 → Rust 同步 | 真实表单保存 `user/web-cross-client`，Rust 下载 hash `5674359c0d978abfe8d41881f4e6a323c21a207e2ac84bd0e94dbd53815b9f9f` |

客户 Web 页的英文创建流程、中文切换、删除状态已实际观察。删除卡片仅保留历史入口。Rust Web 页也通过真实 Runtime 创建，并由 CLI 读回同一版本。

官方管理员 Web 已实际走完预览、确认发布、升级、历史回退和目录撤回。预览前后公开目录不变；确认发布 rc1 后两端验签 hash `70c98225efa2d1abeb257c9acf568552ecccacce3a88c7b19ea60036fa0fdae6`。rc2 对应 `a6055b5573f29ab7cbb10c0ffd880c9b342acd155000326b78d24f10c82f2379`；回退后公开目录重新返回 rc1，撤回后目录隐藏该 ID，但旧固定 hash 下载仍通过验签。

## 已完成的回归

| 客户端 | 结果 |
|---|---|
| Rust | 定义 9 项、registry 14 项、子智能体 89 项（2 项既有 ignored）、TUI 240 项、Web 122 项通过；最终 build/fmt/diff 通过 |
| Mac | Registry/ProfileUnification/GoalTeamRole 三套 38 项原生测试全部通过，test_exit=0，日志与 xcresult 数量一致；明暗 SwiftUI 渲染截图已检查 |
| some.im | authapi/agentregistry/migrate 回归 689 个通过记录（含子用例）；真实 MySQL HTTP 9 项及独立八表迁移用例均另行通过；新增前端 7 项、TypeScript 和生产构建通过 |

Rust 的两个相同基础职责定义可分别绑定不同本机模型；绑定、Key 和授权文件集合不进入导出或同步包。实际 Catalog.run 已分别执行 reviewer 和 implementer 的独立 Provider fixture，检查专业指令、revision、完整锁定技能与运行快照。没有写授权集合时，实际 Provider 的工具列表没有编辑、Shell 或嵌套派工权限。签名但不兼容的官方更新候选拒绝替换已有正常安装，保留旧 hash、启用状态、pin、绑定和历史。

已沿实际 Runtime 调用链核对刷新时机：`daemon/task_manager.rs` 每次调用 `harness::execute_runtime`，后者重新 build，读取当前安装选择和本机绑定。管理操作影响后续回合新建的派工目录，已经运行的子任务继续使用自己的不可变快照。

Mac 原生 Runner 也分别运行 reviewer 和 implementer 的 Provider fixture，确认完整技能正文与固定 hash 实际发出。pin/回滚、停用状态、网络响应丢失后的稳定幂等、错误写回包、账号切换及初次 share 保留本机绑定均已验证。未执行 Mac 全仓测试，当前证据覆盖新增定义库及其职责/权限适配相关套件。

签名密钥 ID 改为公钥 hash 派生，固定定义 hash 不变，旧版本下载由当前密钥重新签名。两端保留同 origin 的独立旧 key ID 以使用离线 pin，拒绝同 ID 不同公钥；Rust 额外验证不同 origin 不混合可信键。目录撤回及停止旧密钥签发不承诺强制擦除客户端缓存。最终服务真实读回版本 header `0.248.0-rc2`，未登录私有及管理员入口均返回 401；两端再次下载验签相同官方 hash，通过第二账号隔离与重复同步检查。

## 待发消息回归

截图问题的原因是 TUI 用户队列调度要求先消耗内部待唤醒事件，而回合终态没有保证产生下一次唤醒，因此用户队列一直留着；状态栏只检查 running，提前显示“轮到你”。

修复后，用户提交的待发消息可直接启动下一轮，内核事件仍保留并在模型边界处理。队列/附件编辑面板继续暂停发送；有待发内容时显示等待发送，不催用户再次输入。独立 bugfix 工作树版本 `0.100.0-rc2` 的 14 个队列测试及 240 个相关 TUI 测试全部通过，修复也已集成到 `0.101.0-rc1` 功能工作树并重跑 14 项测试。

本次没有替换用户正在运行的旧二进制，因此其内存中的队列不能由源码修改自动发送。截图里的 SSO 问题只作为待发消息症状证据，没有将附件内容当作新的执行指令。

## 验收边界

本次验证产品行为、权限与版本一致性；Provider trial 为独立 fixture，没有花费真实模型配对评测，也不把普通运行完成计为质量提升。图片 Worker、团队共享和 RSI 候选/晋升仍按后续阶段处理。

## 交付状态

代码已保存在三个隔离功能分支的本地提交中。Mac `7963851f`；some.im 功能 `3f1138cb`，签名轮换 rc2 修复 `9140099f`；独立队列修复 `e8df5eb`。Rust 本批提交同时包含本报告与共享契约。

尚未合并、推送、发版或生产迁移；没有覆盖用户原有 dirty checkout、Mac 应用或 CLI 安装。生产官方分发需要配置稳定签名环境。实现通过后的正式发布与安装仍需单独交付证据。

本次本机验收 HTTP、Vite 和独立 MySQL 容器已停止；数据、测试报告、源码工作树与截图保留供审查，未重启用户正在运行的 Runtime/TUI。

结构化记录见 [JSON 报告](agent-registry.json)。
