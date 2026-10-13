# 手机运行配置协议与联调

更新日期：2026-10-13。

## 配置与回显

- `session.configure`：必须指向已存在会话，payload 为局部更新，字段为 `approval_mode`、`provider_id`、`model`、`skills`、`experts`、`plugins`。
- 所有字段先校验后应用；成功 `ack`，失败 `error`，失败不提交消息。不上传密钥、API 地址或本机技能路径。
- 多选字段缺省表示保持，空数组表示清空。手机仍可随 `message.send` 携带配置，新宿主必须校验并应用，不能静默忽略。
- `capabilities.get` 与 `capabilities.updated` 带 `session_id`；列表包括供应商、模型、技能、专家、插件及 `is_active`。手机丢弃旧会话的能力回包。
- `active_approval_mode`、`active_provider_id`、`active_model_id` 表示实际状态；`supports_configuration: true` 表示独立配置命令可用。
- 审批规范值：`strict`、`smart`、`workspace-write`、`full-access`；继续接受已发布别名。`read-only` 是工作区策略，不是第五个可选档位。

## 宿主语义

| 项目 | Mac | Rust Runtime |
| --- | --- | --- |
| 供应商 | 本机供应商库 | 当前会话 config 中所有 profile，补全 workspace/default profile |
| 模型 | 供应商及已知模型目录 | 查询所选供应商 `/models`，补全已配置和当前模型；失败 `models_status=unavailable` |
| 技能 | 所有启用技能，无额外 80 项截断 | 实际 SkillCatalog，遵守工作区 skills 允许范围 |
| 专家 | 原生专家包 | 暂无同等机制，列表为空，非空选择明确报错 |
| 插件 | 已启用插件技能组 | 仅已批准且启用插件；选择进入执行附件，不能远程批准或启用新插件 |
| 审批 | 更新真实会话审批模式 | 更新会话及正在执行的审批句柄；只读工作区继续强制只读 |
| 供应商和模型生效 | 实际 composer，供下一次发送 | 持久化 Runtime 会话，供下一轮 claim 使用 |
| 技能生效 | 附加/取消真实 pending skills | 正文经已有文本附件通道进入提交轮次，用户气泡保留原始文本 |

## 验证要求

自动化覆盖：四档映射、旧别名、配置回读、取消选择、非法字段拒绝、配置持久化、实际提交上下文、本机 `/models` 服务、真实 WebSocket 中继以及 Android 解析宿主输出。

真机验收需在线 ADB 设备与匹配的新宿主版本。未完成安装、扫码及手机界面操作时，自动化联调不能记作真机验收通过。提交、合并、推送、安装和发布需分别记录。
