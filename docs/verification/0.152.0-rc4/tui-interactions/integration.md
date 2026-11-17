# TUI 交互真实终端验收

willdeep 0.152.0-rc4

- compact / model waiting is visible before output：PASS
- compact / streaming updates new-content feedback and preserves the visible history anchor：FAIL

结果：FAIL
RuntimeError: reading indicator contains stale cells: "\u2594New content \u00B7 End to latest                                                                       \u2594"

私有 HOME、工作区、假 Provider 与独立 PTY；模型请求仅发往本机合成服务。
剪贴板隔离：私有进程的 SSH_CONNECTION 强制 OSC 52，输出仅由夹具捕获，不转发到宿主终端或剪贴板。
动态证据：捕获 0 次复制，其中 0 次与完整原文精确一致。各次复制的预期原文、实际文本、字节数及缩放后的应用帧边界见 integration.json；未捕获不算通过。
