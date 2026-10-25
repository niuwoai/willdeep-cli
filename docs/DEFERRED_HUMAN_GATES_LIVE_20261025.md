# 人工等待与队列真实多端联调（2026-10-25）

Android 真机、Web 浏览器、TUI 已通过本轮联动验收。**Mac 真实 UI 验收待完成，用户已选择稍后手动验收。**

版本：Rust 0.116.0-rc2；Android 1.27.0-rc2 (128)；Mac 1.424.0-rc1 (936)。未提交、未推送、未发布。

## 真实链路

Android SDK 36 真机通过 USB 反向端口连接回环 WebSocket 中继；中继仅转发信封，实际持久化、分类、调度和恢复均由隔离 Runtime 执行。Web 使用真实浏览器，TUI 使用 QTerm 真实终端。模型为本地确定性 HTTP/SSE 夹具，不代表生产模型独立性分类质量。

## 已验证

- Web failed submission preserved draft; reconnect retry created one durable queue entry
- Web and TUI recovered the same original question across client disconnect/restart
- Dependency at queue head stayed queued; independent TUI submission edited on Web executed once
- After two independent tasks, original exact question was answered while side task ran; resumed at safe boundary
- Real Android ViewModel and Compose queue/approval components connected to real Runtime through USB reverse
- Android reconnect retained queue ID and original gate ID; Android edit appeared on Web
- Web edited the Android queue entry; Android received same ID with updated text
- Android independent task completed before original; original answer completed original turn; queue empty
- 5-second test timeout parked gates without answering; production default remains 300 seconds

最终真实请求与轮次状态：[runtime-final.json](/private/tmp/willdeep-deferred-gates-runtime/build/deferred-live-20261025/runtime-final.json)。原问题均由明确的 yes 回答解决，所有独立执行完成。

## 修复

- Gate ownership now follows explicit selected session across clients; foreign session cannot read or answer gate
- Web activity and decisions carry session identity; stale activity responses after session switch are ignored
- Replay without tool output is pending, not a failed/interrupted tool
- Android opt-in QA applicationIdSuffix prevents overwriting installed user application
- Web queue editor sits above composer with a bounded height; empty queue takes no space

## 测试结果

- Android 真实 Runtime 联动 1/1、队列编辑与发送按钮真机 UI 4/4，通过。
- Android 单测 139/142 通过，3 跳过，0 失败；lint 与 QA APK 构建通过。
- Web 28 文件、167 项通过；构建通过，存在既有 bundle 大小提示。
- Rust 调度单测 8 项、跨端请求投影 6 项通过；真实 Runtime/Web 集成 1 项通过，包含外部会话读写拒绝。
- Mac 本轮相关回归 57/57 通过；这些结果不等同于真实 Mac UI 验收。

## 初次失败与修正

- First Rust integration attempt could not bind loopback under sandbox; elevated isolated rerun passed
- Initial new Android test referenced wrong state field; corrected to conversationMessages before device rerun
- First Android live final assertion overfit fixture answer text; Runtime had completed original. Retest verifies original gate ID, idle session and new reply; full rerun passed

## Mac 待验收

自动化辅助服务在读取 QA 窗口时崩溃；异常为 EXC_BREAKPOINT/SIGTRAP，栈含 Array.remove(at:)。QA 应用采样显示主线程正常等待事件，没有观察到应用卡死。重置连接与重启隔离实例后仍无法可靠操作；用户选择稍后手动验收。

保留 [QA 应用](/private/tmp/deferred-live-qa-20261025/WillDeepQA.app) 和本地模型脚本 `ruby /private/tmp/deferred-mac-provider.rb`。在 QA 窗口选择 `/private/tmp/deferred-live-qa-20261025/workspace`，发送 `ORIGINAL_GATE_MAC`；看到原问题后加入 DEPENDENT 消息，再加入独立消息，验证 5 秒收起、独立执行与原问题恢复。随后还需验证 Android 连接 Mac 原生宿主及重连。不要在正式 WillDeep 中执行这组夹具。

## 证据

- [Android 完整联动原始日志](/private/tmp/willdeep-deferred-gates-runtime/build/deferred-live-20261025/android-live.log)
- [Android 四项 UI 原始日志](/private/tmp/willdeep-deferred-gates-runtime/build/deferred-live-20261025/android-ui.log)
- [Web 收起及跨端编辑截图（布局修复前）](/private/tmp/willdeep-deferred-gates-runtime/build/deferred-live-20261025/web-parked.png)
- [Web 小窗口布局修复后截图](/private/tmp/willdeep-deferred-gates-runtime/build/deferred-live-20261025/web-layout-fixed.png)
- [Web 最终 167 项测试日志](/private/tmp/willdeep-deferred-gates-runtime/build/deferred-live-20261025/web-final-tests.log)
- [Android 真机收起截图](/private/tmp/willdeep-deferred-gates-runtime/build/deferred-live-20261025/android-parked.png)
- [TUI 最终屏幕文本](/private/tmp/willdeep-deferred-gates-runtime/build/deferred-live-20261025/tui-final.txt)
- [Runtime/Web 集成日志](/private/tmp/willdeep-deferred-gates-runtime/build/deferred-live-20261025/rust-live.log)
- [Mac 57 项测试报告](/private/tmp/xedit-deferred-gates-runtime/build/fast-tests/report.json)
- [Mac 自动化崩溃摘要](/private/tmp/willdeep-deferred-gates-runtime/build/deferred-live-20261025/mac-automation-failure.json)
- [Mac QA 采样](/private/tmp/willdeep-deferred-gates-runtime/build/deferred-live-20261025/mac-qa.sample.txt)

布局复测：717px 高视口中，队列编辑区 215.09px，正文区仍有 231.91px；队列位于输入框上方且不嵌入输入框。取消编辑、删除队列及回答原问题均已再次通过真实浏览器操作。Android 临时 QA 包和本任务 USB 反向端口已清理，原应用仍保留；Mac QA 应用与本地模型保留供后续手动验收。

## 边界

- No commit, integration, push, tag or release
- No installed production app replaced
- No normal user Runtime upgrade or restart requested
- Deterministic local model; not production-model classification quality acceptance
- Android UI is instrumentation of production ViewModel and Compose components; full manual navigation is not claimed
- Rust /local and Mac external CLI delegation preserve existing semantics
