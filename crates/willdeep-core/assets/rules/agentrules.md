<!-- bundled from https://github.com/niuwoai/agentrules @ 0.21.0 (compact cut: MUST / MUST NOT only). Regenerate from that repo with `ruby scripts/generate_claude_md.rb --all --compact` when the upstream version changes, then keep this header. -->

# Alma 规则（精简版）

这些是长期生效的硬约束。每条只保留 MUST / MUST NOT；背景、模板和细节在 https://github.com/niuwoai/agentrules 的同名文件里，需要时再读，不要把整库搬进对话。

## AI 协作红线（ai-guardrails）

- **MUST**：创建/更新文件用编辑/写文件能力（直接写入文件、应用补丁），确保变更可审计、可回滚、可 review。
- **MUST NOT**：用管道、`cat`、`echo` 重定向、heredoc 等 shell 方式生成或覆写文件内容。
- **MUST**：破坏性或不可逆操作先向用户确认后再执行：删除数据、覆盖未读过的文件、强制推送、对外发布、执行 `migrate down`。
- **MUST NOT**：`git push --force` 到 main / develop 等共享分支。
- **MUST NOT**：执行不带 WHERE 的 UPDATE / DELETE，或 DROP / TRUNCATE，除非用户明确要求且已确认备份可恢复。
- **MUST NOT**：`rm -rf` 删除仓库外目录或用户数据；批量清理前先列出将删除的内容让用户确认。
- **MUST**：如实报告执行结果。测试失败就说失败并附输出，跳过的步骤明确说明；不把未验证的工作说成已完成。
- **MUST**：发现疑似密钥或敏感信息时优先脱敏或移除并提示风险，不复述到回复、日志或提交信息里。
- **MUST NOT**：为了诊断去读凭据文件。不 `cat`、不 `head` 探测私钥、`.env`、token 文件是否可读。
- **MUST NOT**：做与任务无关的改动（顺手重构、重排格式、改无关命名）。
- **MUST NOT**：把写给开发者的需求说明、验收标准、Prompt 或规则，未经判断就当作最终用户可见文案。
- **MUST NOT**：直接修改带 `GENERATED FILE — DO NOT EDIT` 标记的文件；改源之后重新生成。

## 编程通用（programming）

- **MUST**：一个项目一种代码风格，用 linter 强制。
- **MUST**：用户可见文案走 i18n，不硬编码在视图代码里；业务值与展示文案分离。
- **MUST**：核心逻辑有单元测试，测试数据隔离，不依赖外部环境状态。
- **MUST**：错误信息带上下文（目标、参数、原因），业务异常用自定义类型。
- **MUST**：SQL 一律参数化查询。
- **MUST**：结构化日志，分级正确，敏感信息脱敏。
- **MUST**：关键决策落仓库文档，不只留在聊天记录。
- **MUST NOT**：吞异常（空 catch / 裸 except 不 log）。
- **MUST NOT**：密钥、Token、凭据进代码、日志、示例或提交。
- **MUST NOT**：魔法数字或字符串散落代码，必须抽成有名字的常量。
- **MUST**：单个文件不超过 3000 行；超过 2000 行就要考虑拆分。

## 版本管理（version-control）

- **MUST**：每次修改代码后同步更新版本号和 `CHANGELOG.md`。
- **MUST**：版本号采用语义化格式，允许 rc 预发布标识。
- **MUST**：功能新增才提升正式版本位并把 rc 重置为 `rc1`；bugfix 只递增 rc 号，不提升正式版本位。
- **MUST**：用户要求 git 提交时，打 `v{版本号}` tag 并随代码一起推送。
- **MUST**：敏感文件（`.env`、密钥）必须列入 `.gitignore`，不得提交。
- **MUST NOT**：直接在 main / develop 上 commit；一律走 feature 分支 + PR。
- **MUST NOT**：一个 PR 混 feat、fix 与 refactor。
- **MUST NOT**：改了版本号不写 CHANGELOG，或 CHANGELOG 与代码不同步。

## 前端（frontend）

- **MUST**：新项目默认 React + TypeScript，`strict: true` 全开；包管理器优先 yarn。
- **MUST**：JSX/TSX 不硬编码对外可见文案，一律走 i18n。
- **MUST**：数据请求用 React Query / SWR / RTK Query，不用裸 `useEffect` 拉数据。
- **MUST**：`dangerouslySetInnerHTML` 必须先 sanitize。
- **MUST NOT**：使用 `any`（用 `unknown` + 类型守卫）。
- **MUST NOT**：列表有增删时用 index 做 key。
- **MUST NOT**：token 放 localStorage；后端密钥放进 `VITE_` 前缀环境变量或任何前端可见配置。

## Go

- **MUST**：error 永远放返回值最后一位；wrap 时保留上下文（`fmt.Errorf("...: %w", err)`）。
- **MUST**：错误类型判断用 `errors.Is` / `errors.As`，不比对 `err.Error()` 字符串。
- **MUST**：goroutine 生命周期可控，context 取消 + WaitGroup / errgroup 等待，不泄漏。
- **MUST**：接口定义在使用方 package，不在实现方定义再被 import。
- **MUST NOT**：panic 做流程控制；panic 只给不可恢复错误。
- **MUST NOT**：channel 读端执行 close；谁写谁 close。
- **MUST NOT**：用运行时反射的 DI 框架。

## Python

- **MUST**：一个项目一个虚拟环境，不动系统 Python；依赖锁版本。
- **MUST**：函数签名和公共 API 写类型注解；状态、类型、选项用 Enum，不用裸字符串。
- **MUST**：数据库迁移用 Alembic / Django migrations，生成的脚本人工 review。
- **MUST NOT**：裸 `except:` 或 `except Exception` 不 log 就吞掉。
- **MUST NOT**：async 函数里调用阻塞的同步 IO。
- **MUST NOT**：异常只 print；用 `logger.exception()` 保留 traceback。

## 数据库（migrations / queries / sync）

- **MUST**：所有 DDL 变更走迁移文件，禁止手改数据库；up/down 成对，down 必须实际测试过。
- **MUST**：每张业务表有主键、`created_at`、`updated_at`；所有外键列建索引。
- **MUST NOT**：修改已提交的迁移文件；一个迁移不混多件事。
- **MUST NOT**：`migration down` 在未确认备份和影响范围前执行。
- **MUST**：一律参数化查询；动态表名或列名无法参数化时用白名单校验。
- **MUST NOT**：`SELECT *`；明确列出需要的列。
- **MUST NOT**：把 `password_hash` 等敏感列查出传给前端。
- **MUST**：每条同步操作幂等，可安全重放；有 checkpoint 断点续传。
- **MUST NOT**：跨系统同步追求实时强一致；定义可接受的延迟窗口。

## 部署与服务器（deployment / linux-server）

- **MUST**：每次部署可快速回滚；替换前备份旧产物，至少保留最近 3 个版本。
- **MUST**：新版本通过健康检查后再承载流量。
- **MUST NOT**：静默失败；部署、迁移、重启、健康检查任一步失败必须明确报告原因和下一步。
- **MUST NOT**：配置、密钥、证书打进构建产物。
- **MUST**：SSH 禁用 root 登录和密码认证，只留密钥；防火墙最小开放。
- **MUST**：监控 CPU、内存、磁盘（空间和 inode），超过 80% 告警；备份自动化且定期做恢复演练。
- **MUST NOT**：Docker 镜像用 `latest` 标签，版本 pin 死。
- **MUST NOT**：共用登录账号；一人一号，操作可追溯。
