# TUI 交互真实终端验收

willdeep 0.152.0-rc4

- compact / model waiting is visible before output：PASS
- compact / streaming updates new-content feedback and preserves the visible history anchor：PASS
- compact / the full original selection survives incoming output：PASS
- compact / the full original selection survives resize and copies through isolated OSC 52：PASS
- compact / busy follow-up is accepted without starting another provider request：PASS
- compact / queue exposes the actual priority-send effect：PASS
- compact / accepted follow-up starts exactly once and preserves the composer draft：PASS
- compact / return-to-latest affordance reveals buffered output without sending the draft：PASS
- compact / completed synthetic responses were persisted by Runtime：PASS
- full / model waiting is visible before output：PASS
- full / streaming updates new-content feedback and preserves the visible history anchor：PASS
- full / the full original selection survives incoming output：PASS
- full / the full original selection survives resize and copies through isolated OSC 52：PASS
- full / busy follow-up is accepted without starting another provider request：PASS
- full / queue exposes the actual priority-send effect：PASS
- full / accepted follow-up starts exactly once and preserves the composer draft：PASS
- full / return-to-latest affordance reveals buffered output without sending the draft：FAIL

结果：FAIL
RuntimeError: latest output: timed out
Last terminal rows:
│✓ · Queued messages waiting to send                                                               │
└──────────────────────────────────────────────────────────────────────────────────────────────────┘
┌Prompt · Your turn · Enter sends · Cmd/Ctrl+Enter queues for later · Smart review (Shift+Tab)─────┐
│UNSENT_DRAFT_草稿😀                                                                               │
│                                                                                                  │
│                                                                                                  │
└──────────────────────────────────────────────────────────────────────────────────────────────────┘
✓ · Focus: Chat · context 0% · latest ↑1.00K ↓100 · 0.4s · Enter sends · Cmd/Ctrl+Enter queues · Ctr

私有 HOME、工作区、假 Provider 与独立 PTY；模型请求仅发往本机合成服务。
剪贴板隔离：私有进程的 SSH_CONNECTION 强制 OSC 52，输出仅由夹具捕获，不转发到宿主终端或剪贴板。
动态证据：捕获 2 次复制，其中 2 次与完整原文精确一致。各次复制的预期原文、实际文本、字节数及缩放后的应用帧边界见 integration.json；未捕获不算通过。
