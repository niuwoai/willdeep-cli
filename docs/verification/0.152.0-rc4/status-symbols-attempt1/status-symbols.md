# TUI 状态符号与点击验收

willdeep 0.152.0-rc4

- compact / started message uses a symbol without the old status phrase：PASS
- compact / queued symbol does not claim early start or completion：PASS
- compact / normal terminal status comes from the completed turn：PASS
- compact / mouse-down keeps the complete footer and puts reading feedback on another row：PASS
- compact / drag-selection preserves footer text and draft：PASS
- compact / resize preserves footer text and isolated selection copying：PASS
- compact / return-to-latest preserves the footer and unsent draft：PASS
- compact / started message uses a symbol without the old status phrase：PASS
- compact / queued symbol does not claim early start or completion：PASS
- compact / normal terminal status comes from the completed turn：PASS
- compact / mouse-down keeps the complete footer and puts reading feedback on another row：PASS
- compact / drag-selection preserves footer text and draft：PASS
- compact / resize preserves footer text and isolated selection copying：PASS
- compact / return-to-latest preserves the footer and unsent draft：PASS
- full / started message uses a symbol without the old status phrase：PASS
- full / queued symbol does not claim early start or completion：PASS
- full / normal terminal status comes from the completed turn：PASS
- full / mouse-down keeps the complete footer and puts reading feedback on another row：FAIL

结果：FAIL。RuntimeError: reading feedback after mouse-down: timed out
Last terminal rows:
│Turn completed                                                                                                                    │
└──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
┌Prompt · Your turn · Enter sends · Cmd/Ctrl+Enter queues for later · Smart review (Shift+Tab)─────────────────────────────────────┐
│UNSENT_STATUS_DRAFT                                                                                                               │
│                                                                                                                                  │
│                                                                                                                                  │
└──────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
Turn completed · Focus: Chat · context 0% · latest ↑1.00K ↓100 · 0.3s · Enter sends · Cmd/Ctrl+Enter queues · Ctrl+S select · F1    

full/compact Runtime TUI; status symbols, footer click/drag, resize, isolated OSC 52 copy and unsent draft

Private HOME/WILLDEEP_HOME/workspace; loopback synthetic SSE; no real credentials or clipboard.
