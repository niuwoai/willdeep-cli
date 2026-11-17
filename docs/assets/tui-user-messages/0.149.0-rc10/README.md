# TUI 鲸鱼首屏与用户消息素材

版本：0.149.0-rc10。采集日期：2026-11-16。当前使用彩色鲸鱼首屏，替代 rc9 的纯文字首屏。用于教程或官网选图，尚未对外发布。

## 图片索引

PNG 以 2 倍尺寸保存，便于裁切和高分辨率展示。SVG 保留单元格原稿；半块像素按单元格矩形还原，不依赖字体中的块字符字形。

| 模式 | 终端尺寸 | PNG 像素尺寸 | PNG | SVG 原稿 |
| --- | --- | --- | --- | --- |
| 简洁模式 | 120×45 | 2464×1864 | [compact.png](compact.png) | [compact.svg](../../../verification/0.149.0-rc10/user-messages/compact.svg) |
| 完整模式 | 120×45 | 2464×1864 | [full.png](full.png) | [full.svg](../../../verification/0.149.0-rc10/user-messages/full.svg) |
| 窄屏 | 60×45 | 1264×1864 | [narrow.png](narrow.png) | [narrow.svg](../../../verification/0.149.0-rc10/user-messages/narrow.svg) |

简洁模式适合介绍对话布局；完整模式适合展示活动区；窄屏适合说明终端宽度变化。画面高度使用 45 行，保留完整鲸鱼，避免对话增多后顶部滚出视口。用户消息位于右侧蓝灰背景块，不展示生成的 `You:` 前缀，保留正文、发送时间和回复间距。

鲸鱼边界验收另用 80 行的独立欢迎会话，简洁与完整布局各完成 11 个检查，覆盖 26→25 列、14→13 列的尺寸切换，以及缩窄至 10 列后恢复到 120 列。逐格比对预期图案，尺寸变化后必须收到新的绘制输出；同时验证 7 行结构、终端软折行与左右边界。对应原始记录保存在验收目录，不把 60 列聊天图作为极窄宽度修复的唯一证据。

边界预览：[25 列简洁模式](../../../verification/0.149.0-rc10/user-messages/whale-compact-4-25.png)、[13 列完整模式](../../../verification/0.149.0-rc10/user-messages/whale-full-8-13.png)。三套主图和这两张边界图均已逐张检查。每套主图包含原宽版鲸鱼的 99 个块单元格，PNG 实际尺寸和 SVG 单元格几何均已核对。

建议替代文本：终端顶部显示彩色鲸鱼、产品名与版本；用户消息位于右侧蓝灰背景块，显示正文和时间，助手回复位于左侧。

## 来源与使用说明

- 图片来自实际 debug TUI 与 Runtime 的隔离演示对话，采集 ANSI 单元格后还原为 SVG，再转换为 PNG；不是操作系统窗口截图，文字字体和终端调色板可能不同。
- 鲸鱼的 `▀`、`▄`、`█` 按对应半格或整格矩形绘制，前景与背景颜色直接取自终端单元格，避免字体替换使像素错位。
- 使用独立临时 home、工作区和本机回环服务。演示对话没有真实会话内容、真实凭据或外部模型请求。
- `fixture-chat-model` 是本机固定回复的测试模型标识，不代表真实模型能力或背书。
- 原始 ANSI、终端尺寸变化记录、JSON 与 Markdown 验收报告保存在[验收目录](../../../verification/0.149.0-rc10/)。
- 素材入库不等于官网或教程已发布；正式选用时按页面需要设置替代文本，并保留版本与来源说明。

## 复现

在仓库根目录构建本轮 debug CLI：

```bash
CARGO_INCREMENTAL=0 cargo build -p willdeep
```

```bash
ruby scripts/verify_tui_user_messages.rb target/debug/willdeep docs/verification/0.149.0-rc10/user-messages
```

使用 `rsvg-convert` 导出 2 倍尺寸 PNG，例如：

```bash
rsvg-convert --zoom 2 --output docs/assets/tui-user-messages/0.149.0-rc10/compact.png docs/verification/0.149.0-rc10/user-messages/compact.svg
```
