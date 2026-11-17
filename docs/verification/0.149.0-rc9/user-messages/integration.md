# TUI 用户消息验收

版本：0.149.0-rc9。真实 TUI + Runtime，独立临时 home，仅本机模拟模型。

- compact（120 列）：PASS
- full（120 列）：PASS
- narrow（60 列）：PASS

验证纯文本 willdeep 标识、无像素鲸鱼、隐藏生成的用户角色前缀、保留正文和时间、用户模型过滤、右侧背景块、角色间距、助手模型保留。SVG 从实际 ANSI 单元格还原，字体与终端调色板可能不同。
