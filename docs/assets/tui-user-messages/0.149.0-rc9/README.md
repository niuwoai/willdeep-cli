# TUI 用户消息排版素材

版本：0.149.0-rc9。采集日期：2026-11-16。当前为纯文本 `willdeep` 标识版，替换同路径下原有的像素鲸鱼图片。用途：教程插图或官网配图的候选素材，尚未对外发布。

## 图片索引

PNG 以 2 倍尺寸保存，便于裁切及高分辨率展示。使用 PNG 可固定字体效果；SVG 保留可缩放原稿，但字体取决于查看设备。

| 模式 | 终端尺寸 | PNG 像素尺寸 | PNG | SVG 原稿 |
| --- | --- | --- | --- | --- |
| 简洁模式 | 120×35 | 2464×1534 | [compact.png](compact.png) | [compact.svg](../../../verification/0.149.0-rc9/user-messages/compact.svg) |
| 完整模式 | 120×35 | 2464×1534 | [full.png](full.png) | [full.svg](../../../verification/0.149.0-rc9/user-messages/full.svg) |
| 窄屏 | 60×35 | 1264×1534 | [narrow.png](narrow.png) | [narrow.svg](../../../verification/0.149.0-rc9/user-messages/narrow.svg) |

简洁模式适合介绍对话布局；完整模式适合展示活动区；窄屏适合说明终端宽度变化。首屏采用青色小写 `willdeep` 与灰色版本号，不依赖半块字符或终端单元格宽高比。图片同时展示隐藏用户角色前缀后的正文、发送时间、右侧背景块与回复间距。

建议替代文本：终端顶部显示 willdeep 文字标识与版本号；对话中的用户消息位于右侧蓝灰背景块，仅显示正文和发送时间；助手回复位于左侧，两者之间留有空白。

## 来源与使用说明

- 通过实际 debug TUI 与 Runtime 交互采集 ANSI 单元格，先还原为 SVG，再转换为 PNG；不是操作系统窗口截图。字体和调色板可能与用户终端不同。
- 对话为隔离夹具中的演示内容，`fixture-chat-model` 是本机固定回复的测试模型标识，不代表真实模型测试或背书。
- 使用独立临时 home、工作区和本机回环服务，没有真实会话内容、凭据或外部模型请求；界面中的临时工作区名称属于夹具。
- PNG 未额外修饰或改写界面文字，SVG、原始 ANSI、尺寸变更记录与 JSON/Markdown 验收报告一并保留在[对应验收目录](../../../verification/0.149.0-rc9/user-messages/)。
- 本目录用于存放候选素材；官网或教程正式选用时，应保留版本与演示来源说明，并按页面需要设置替代文本。

## 复现

在仓库根目录，先构建 debug CLI 并运行真实终端验收：

```bash
CARGO_INCREMENTAL=0 cargo build -p willdeep
```

```bash
ruby scripts/verify_tui_user_messages.rb target/debug/willdeep docs/verification/0.149.0-rc9/user-messages
```

PNG 使用本机 `rsvg-convert` 从对应 SVG 按 2 倍尺寸导出，例如：

```bash
rsvg-convert --zoom 2 --output docs/assets/tui-user-messages/0.149.0-rc9/compact.png docs/verification/0.149.0-rc9/user-messages/compact.svg
```
