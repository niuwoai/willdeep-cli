# 人工等待与 Runtime 队列验证

时间：2026-10-24T18:21:03Z

分支：`codex/deferred-gates-runtime`。Rust 0.116.0-rc1；Mac 1.424.0-rc1 (936)；Android 1.27.0-rc1 (127)。

工作树：

- [rust](/private/tmp/willdeep-deferred-gates-runtime)
- [mac](/private/tmp/xedit-deferred-gates-runtime)
- [android](/private/tmp/android-deferred-gates-runtime)

## 验证结果

- Rust 全套：
  - `test result: ok. 883 passed; 0 failed; 3 ignored; 0 measured; 0 filtered out; finished in 835.12s`
  - `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 18 filtered out; finished in 0.01s`
  - `test result: ok. 18 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 7.78s`
  - `test result: ok. 40 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 271.06s`
  - `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s`
  - `test result: ok. 37 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s`
  - `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s`
- Rust 最终相关回归：退出码 0，结果见 JSON。
- Rust 全量首次 JSON 汇总失败（未加载 time 库）；本报告依据完整测试日志重建，全量命令退出码未记录。最终定向回归单独记录退出码 0。
- Mac 首次全量：5083 项，5075 通过、7 失败、1 跳过。
- Mac 最终相关回归：87 通过、0 失败、0 跳过。
- Mac 初次全量的 7 项问题均已修正并复测通过：1 项夹具时间精度、2 项旧行为断言、4 项共享数据缺失。首次原始 xcresult 与失败记录已归档供复核。
- Android：139 通过，3 跳过，0 失败；lint 与 APK 编译通过。
- Web：27 个文件、165 项测试通过，生产构建通过。

## 已核对行为

- Android 网关和控制 API 读写同一持久队列；跨端编辑、删除和附件保持身份
- 真实临时 Runtime 跳过依赖队首，连续处理两条独立消息，恢复同一个原问题
- 真实 Web HTTP 队列读写、旧文本冲突拒绝、重复提交去重和会话权限检查
- 原审批和多选问题恢复到原始等待句柄，独立执行不覆盖原核心会话及租约
- 超时后即使所有消息都无法确定依赖，仍收起原卡片并保留队列与等待句柄
- 独立执行期间收到的旧答案在安全边界投递；停止后抑制自动认领和恢复
- Web 收起恢复保留回答草稿，队列消息被另一端启动或删除后保留编辑草稿
- Mac 队列与子会话引用持久化、服务商并发名额释放和重新获取、活动组保留
- 独立执行启动失败时仍保留可见用户原文；Mac 先保存正文及附件再提交队列交接

## 验证边界

- 未发布、未推送版本标签、未安装替换应用、未升级或重启现有 Runtime
- 真实 Runtime 和 Web 集成使用隔离 HOME 与回环模拟模型，没有调用生产模型
- 没有将单测和模拟协议快照等同于 Android 真机与多设备真实 UI 验收
- 默认 Rust Runtime TUI 覆盖同步与调度；显式 /local 离线执行保留原有本地执行语义
- Mac 独立调度当前覆盖 WillDeep 原生执行链，外部 CLI 委派消息继续沿用原执行流程

Mac 既有 RSI 测试依赖共享 suite，本次从 Rust 工作树补齐缺失文件，未修改批准基准；canonical SHA-256 为 `281f7dadf99f4cf86c4de5a922cdac7531c933654f878bb6b8f532d502a96575`。位置及来源见 JSON。

[原始 Rust 日志](/private/tmp/willdeep-deferred-gates-runtime/build/deferred-gates/rust.log)；[Mac 首次全量日志](/private/tmp/xedit-deferred-gates-runtime/build/deferred-gates/mac-full-initial.log)。Mac xcresult 路径及最终复测日志保存在对应 JSON 报告中。
