# 完全 UI / 简洁 UI 配置验收

日期：2026-10-21。此记录为源码及隔离环境验收，不代表已经发布或替换用户正在运行的客户端。

## 配置约定

```toml
[ui]
mode = "full" # full 完全 UI（默认）；compact 简洁 UI
compact_status_line = true # 简洁 UI 底部信息行，默认显示
```

- 两种 UI 固定使用所选模式，不再按闲置时间、键盘或鼠标活动自动切换。
- 简洁 UI 的输入区及信息行背景覆盖终端全部列，包括空白行、右边缘和侧栏下方；信息行不换行。
- 信息行复用完全 UI 的状态、焦点、上下文、Token、缓存、用时与快捷键；关闭常规统计仍会展示临时警告和操作结果。
- 侧栏、队列、审批、搜索和其它弹窗共用交互层，保留输入、附件和任务状态。
- willdeep-config 0.5.0-rc1 提供相同字段的中英文表单，保存后重新打开客户端生效。内置副本与独立插件源码一致；安装新版保留旧包和配置，不沿用旧内容的权限审批。

## 验证

- CLI 完整单元测试串行复测：815 通过、3 忽略、0 失败。首次并行运行中，Web 监听关闭测试出现一次时序断言失败；串行完整复测包含该测试并通过，未修改无关 Web 服务源码。
- 严格 Clippy：`cargo clippy -p willdeep --bin willdeep --all-targets -- -D warnings` 通过。
- 配置插件：128 项测试、953 项断言通过；包含双语控件、默认值、模式验证、保存和取消设置，原密钥与未知配置保持不变；页面和包结构检查通过。
- 格式、源码规模、版本一致性、`git diff --check` 通过。

真实 PTY 验收使用独立 WILLDEEP_HOME、Runtime 与模拟 Provider，每次只发一个请求，核对任务完成及最终回答落盘。场景覆盖初始界面、运行中、输入首字符、闲置、点击、宽/窄缩放、侧栏开/关、队列、模型完成后的界面。

三种配置各 11 个阶段，共 33 个阶段通过：简洁 UI 显示信息行、简洁 UI 隐藏信息行、缺省配置的完全 UI。三个模拟任务均完成且最终回答落盘；首字符和草稿保留。

```sh
ruby scripts/quiet_mode_tui_verify.rb /absolute/path/to/willdeep /tmp/ui-compact-visible compact true
ruby scripts/quiet_mode_tui_verify.rb /absolute/path/to/willdeep /tmp/ui-compact-hidden compact false
ruby scripts/quiet_mode_tui_verify.rb /absolute/path/to/willdeep /tmp/ui-default-full default true
```

脚本输出 JSON、Markdown 及各阶段终端帧。等待以真实帧及 Runtime 完成状态为准，不能把输入提示中的完成标记当成模型已经完成。验收不修改真实 config.toml，不停止原 Runtime 或重发用户消息。

## 后台更新提醒

查询仅访问公开 Release 元数据，5 秒超时，响应体上限 512 KiB；真实接口 20 条记录约 217 KiB，格式与上限实测匹配。初次延迟 30–90 秒，后续间隔 6–12 小时，失败同样持久化下次检查时间；文件锁避免多客户端重复查询。数字 rc 顺序、稳定/预发布渠道、缓存、不重复请求、失败退避和通知延后/去重均由隔离测试覆盖。关闭 TUI 时取消后台检查，不影响 Runtime。

官网资料页与独立配置插件另仓库验证、提交和发布，不能将 CLI 标签推送等同于站点部署或本机升级。
