# 原生 mock → 独立宿主复现验收

8 个原生案例通过 116 项绑定核对，host 8/8 通过，实际执行可信 fixture/reference 共 28 次。

| 平台 | 案例 | 原生状态保留 | 绑定检查 | 宿主复现 | 宿主通过 |
|---|---|---|---|---|---|
| macos-native-worker | count-zero-bug | unverified | 13 | verified | true |
| macos-native-worker | count-zero-clean | unverified | 13 | not_applicable_clean | true |
| macos-native-worker | copy-boundary-bug | unverified | 13 | verified | true |
| macos-native-worker | copy-boundary-clean | unverified | 13 | not_applicable_clean | true |
| macos-native-worker | duplicate-first-bug | unverified | 13 | verified | true |
| macos-native-worker | duplicate-first-clean | unverified | 13 | not_applicable_clean | true |
| rust-native-worker | count-zero-bug | unverified | 19 | verified | true |
| rust-native-worker | count-zero-clean | unverified | 19 | not_applicable_clean | true |

原生 observation 保留 unverified，host verdict 独立写入本报告；不覆盖任何原始文件或旧失败记录。核对原 baseline revision/指令、共享契约 bytes/指令、raw 原文 hash、host evidence 一致性、root/worker 血缘、实际 read_file 收据与文件未变。Rust 原始 Worker context 绑定编译 prelude；request_context_projection 明确为首已准入请求派生投影，实际 effective-prompt 与每个 mock receipt 的 system hash 另行核对。

资源案例 6 个，仅检查停止/物理请求边界及未知 usage 保留，不做宿主复现评分（包括用于验证四请求足够完成的资源对照）。

模型调用 0、费用 0；只证明公开 mock 工程链路，不构成能力提升或晋升结论。
