# 0.147.0-rc3 整合验收（2026-11-07）

## 来源与覆盖范围

基线为 `2a2fe3b8277120e269639dd9c7bdc7c4a2b4c862`；完整来源提交为 `f246a18e935dd80bc91edebd3e2dae6e8c1898b5`。整合补充回归、窄屏弹层修复、Web handler 拆分及 RuntimeConnection 装箱后，按以下依赖拆分：

- 思考等级：`8fb4c7f26ab5e9c9b1da0dc93b47532cfea3fba4`（0.147.0-rc1）。
- 计划卡更新：`859033e073f6c94e83fd98b3cb27f74285223c1e`（0.147.0-rc2）。
- 审批与队列：`8013035199c0b8d9c7994910a3d29decca4da95c`（0.147.0-rc3）。

本轮在独立工作树运行，未使用真实上游或生产凭据。测试快照与第三组提交的 `crates`、`web/src` 共 896 个文件逐文件 SHA256 对照，无缺失、额外文件或内容差异。前两组提交另各自通过 Web 构建和 `cargo check --workspace --all-targets`。本文件是新增的本轮验证记录；同目录 README 和旧 results.json 仍是独立的历史证据。

## 本地结果

| 检查 | 结果 |
| --- | --- |
| Core 定向单测 | 50/50 |
| CLI 会话、审批、队列、思考等级、计划、配置单测 | 78/78 |
| Web 六个测试文件 | 37/37 |
| 配置 Web bridge | 6/6 |
| Runtime 独立任务越过依赖队首后恢复原问题 | 1/1；装箱修复后复跑通过，不重复计数 |
| 全 workspace / all-targets Clippy，`-D warnings` | 通过 |
| rustfmt、差异空白检查、版本一致性 | 通过 |
| 源码 3000 行门禁 | 通过；Web handler 拆分后入口为 2990 行 |
| Web 构建 | 通过；保留既有大 chunk 提示 |
| Web lint | 0 错误，1 条既有 ModelPicker hook 依赖警告 |

新增回归覆盖跨 Provider fork 的等级兼容性、持久化、源会话不变，以及选择器关闭后重开刷新映射。

首次检查发现 Web 入口 3036 行、大枚举 Clippy 警告；拆分 handler、装箱连接并补齐 main 构造点后复验通过。没有忽略失败门禁。

## 真实浏览器操作

隔离 Runtime、临时工作区和确定性本地 Provider 下验证：

- 原审批保持待处理且入口可见。
- 普通 Enter 保留草稿，队列仍为 1，并提示处理原请求或明确入队。
- Cmd+Enter 成功后清空草稿，队列变为 2，反馈明确尚未交给当前模型。
- 思考等级设置为 deep，GET 会话回读 deep，响应状态 200，`X-App-Version=0.147.0-rc3`。
- 刷新后 deep 和队列 2 保持。
- 375×812 视口下，窄屏修复后的弹层边界约为 left=12、top=553.10、right=363、bottom=800，全部位于屏内；已查看截图确认无裁切。
- 观察期间控制台无错误；夹具就绪前有预览自动重试，不能据此宣称网络日志全程无失败。

## 证据边界

- 本地没有运行全 workspace 测试全集，也没有执行原生 Linux/Windows 测试；跨平台结果由各 PR 和发布 CI 提供。
- 浏览器交互验证先于仅改变连接存储方式的装箱修复；装箱后另通过严格 Clippy 和真实 Runtime 集成复跑。
- 本记录截止于发布前验收，不证明 tag、公开 Release、安装或生产 Runtime 升级完成；部署结果独立记录。
- 未新增数据库迁移。

机器可读结果见 [integration-review.json](integration-review.json)。
