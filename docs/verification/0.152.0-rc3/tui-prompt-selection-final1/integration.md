# TUI 输入区拖选验收

willdeep 0.152.0-rc3

- compact / Chinese, emoji and cross-line selection visibly highlighted：PASS
- compact / Esc clears selection and preserves unsent draft：PASS
- compact / y/q are typed into draft instead of chat selection commands：PASS
- full / Chinese, emoji and cross-line selection visibly highlighted：PASS
- full / Esc clears selection and preserves unsent draft：PASS
- full / y/q are typed into draft instead of chat selection commands：PASS
- compact-ssh / Chinese, emoji and cross-line selection visibly highlighted：PASS
- compact-ssh / Esc clears selection and preserves unsent draft：PASS
- compact-ssh / Ctrl+C sends exact original selection through OSC 52 and keeps draft alive：PASS
- compact-ssh / y/q are typed into draft instead of chat selection commands：PASS
- full-ssh / Chinese, emoji and cross-line selection visibly highlighted：PASS
- full-ssh / Esc clears selection and preserves unsent draft：PASS
- full-ssh / Ctrl+C sends exact original selection through OSC 52 and keeps draft alive：PASS
- full-ssh / y/q are typed into draft instead of chat selection commands：PASS

结果：PASS。

独立临时配置，未提交提示词，未写入系统剪贴板；SSH 夹具直接截获 OSC 52 并核对原文 payload 与已发送提示，弹层优先级与其余复制决策由 Rust 单元测试验证。
