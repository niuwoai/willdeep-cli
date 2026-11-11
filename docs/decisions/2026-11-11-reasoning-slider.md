# 思考等级：内置档位与 Web 拉杆

> 2026-11-11 · 0.148.0-rc1

## 背景

思考等级原先只有一套「Provider 映射」模型：`[providers.X.reasoning]` 里写逻辑名到上游值的对应表。没有配置映射的 Provider（包括截图里那种只用 `glb-5` 直连的档案）在 Web 端只剩一句「尚未配置思考等级映射」，连选择控件都不出现；TUI、控制 API、Web 路由和 `ProviderConfig::validate` 又会各自拒绝未配置的等级。

用户要求：即使没配映射，也能在 Web 输入框下方强行指定 `low` / `medium` / `high` / `xhigh`，并且选择器做成拉杆。

## 决策

### 1. 内置阶梯只在显式选择时生效

Core 新增 `BUILTIN_REASONING_LEVELS = ["low", "medium", "high", "xhigh"]` 与 `ProviderConfig::effective_reasoning()`。优先级固定为：

1. `non_thinking`（受限结构化辅助请求）—— 任何等级都不写；
2. 会话显式选择；
3. 配置映射里的 `default_level`；
4. 都没有 —— 不写任何字段。

没有映射且用户没选时，请求与旧版本逐字节一致；这是不破坏既有契约的关键。配置了非空映射时，映射是唯一集合，内置档位关闭。

### 2. 一个判定，四处共用

`willdeep_core::provider::accepts_reasoning_level(profile, level)` 与 `reasoning_levels(profile)` 是唯一事实源。TUI `/reasoning`、控制 API、Web 适配器、harness、TUI 启动同步、`/model` 切 Provider、fork 继承都改用它，避免「同一函数、不同配置源」再次分叉。

### 3. 按方言默认路径发送

内置档位的值就是等级名本身，写到各协议的默认参数路径：Chat Completions `reasoning_effort`、Responses `reasoning.effort`、Anthropic Messages `output_config.effort`（并沿用仓库既有的 `thinking: {type: "adaptive"}` 联动）。Chat Completions 的「本地端点默认关思考」在显式选了内置档位时让位，不再叠发 `think: false`。

### 4. Web 拉杆

第 0 档是「跟随 Provider 默认」（发 `null`），其余档位来自配置映射或内置阶梯。指针松手或失焦立即提交，键盘方向键停 350ms 再提交；请求串行化，保存期间控件保持可用，只保留最后一个目标值；失败回滚到已确认值并显示上游原因。原生 `<input type="range">` 自带 role=slider、方向键与 Home/End，触控高度 44px。

配置映射的自定义名字没有天然顺序：映射值全是数字时按数字升序，全是标准 effort 词表（`minimal` / `low` / `medium` / `high` / `xhigh` / `max`）时按该词表，否则保留下拉。切 Provider 后留在会话里的旧等级仍单独成档，但界面会标成「不可用」并说明下一轮不会应用——后端只接受当前 Provider 的映射或内置四档，静默过滤掉旧值；拉到其他档位或「跟随默认」即可清除。

### 5. 失败面

上游拒绝某个值时不会重试、不会静默降级：这一轮直接失败并显示 provider 的错误；把拉杆拉回「跟随默认」或配置显式映射即可恢复。

## 边界

- `xhigh` 在仓库里没有上游先例。公开文档显示 Anthropic 新模型与 OpenAI `gpt-5.1-codex-max` 之后支持，老模型会返回 400，因此文档与界面把它当作「可能被上游拒绝」的档位。
- 首条消息发出前会话尚未建立等级，Chat 请求也不带 `reasoning_level`；此时按 Provider 默认执行，拉杆不会假装已生效。要覆盖首条消息需要扩协议，留待后续版本。
- Web 适配器的预检仍读 Core 会话的配置，控制 API 读 Runtime 会话的配置；两者在正常路径下一致，内置档位的接受结论也与配置源无关。
