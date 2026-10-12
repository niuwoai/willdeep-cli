# 模型行为评测 · glm-5

跑于 2026-10-08T20:15:51Z · 代码 `e98faf2`（工作区不干净） · willdeep 0.94.0-rc1

| 指标 | 值 |
|---|---|
| 任务 | 20（执行 20 · 跳过 0 · 出错 0） |
| **verifier 通过率** | 85.0%（17/20） |
| 作弊 / 超时 | 0 / 1 |
| 误报完成率 | 10.0% |
| **人话率** | 69.6% |
| 思维链占比 | 72.1% |
| 模型调用 / 静默工具轮 | 161 / 49 |
| 工具调用 / 失败 | 138 / 0 |
| token / 秒 | 1485518 / 2107.0 |

| 类型 | 执行 | 通过 | 通过率 |
|---|---:|---:|---|
| fix | 8 | 8 | 100.0% |
| feature | 6 | 5 | 83.3% |
| test | 4 | 2 | 50.0% |
| lint | 2 | 2 | 100.0% |

| 任务 | 类型 | 状态 | 声称完成 | 调用 | 人话率 | 工具调用 | 秒 | 备注 |
|---|---|---|---|---:|---|---:|---:|---|
| js-add-test-clamp | test | passed | 否 | 15 | 60% | 12 | 86.6 | 退出码 5；unverified；变异抓住 2/2 |
| js-flatten-depth | feature | passed | 否 | 14 | 64% | 11 | 59.3 | 退出码 5；unverified |
| js-parse-query-decode | fix | passed | 否 | 16 | 62% | 12 | 160.0 | 退出码 5；unverified |
| python-add-test-normalize | test | passed | 是 | 4 | 100% | 4 | 107.5 | 变异抓住 2/2 |
| python-backoff-delays | feature | passed | 是 | 4 | 75% | 4 | 51.9 |  |
| python-mutable-default | fix | passed | 是 | 4 | 100% | 4 | 122.9 |  |
| ruby-add-test-leap-year | test | failed | 是 | 3 | 67% | 3 | 98.0 | 变异抓住 0/2 |
| ruby-cart-nil-quantity | fix | passed | 否 | 12 | 67% | 9 | 125.0 | 退出码 5；unverified |
| ruby-csv-quoted-comma | fix | passed | 否 | 9 | 89% | 5 | 124.0 | 退出码 5；unverified |
| ruby-slug | feature | passed | 否 | 11 | 73% | 8 | 110.2 | 退出码 5；unverified |
| rust-add-test-median | test | timeout | - | - | - | - | 300.1 | 变异抓住 2/2 |
| rust-checked-factorial | fix | passed | 是 | 6 | 100% | 4 | 40.6 |  |
| rust-clippy-clean | lint | passed | 是 | 9 | 67% | 10 | 49.4 |  |
| rust-dedup-order | feature | passed | 是 | 10 | 80% | 11 | 152.3 |  |
| rust-deny-unused | lint | passed | 是 | 7 | 57% | 6 | 67.6 |  |
| rust-last-n-panics | fix | passed | 是 | 9 | 44% | 8 | 137.9 |  |
| rust-parse-duration | feature | passed | 是 | 9 | 100% | 8 | 117.8 |  |
| rust-parse-port-result | fix | passed | 是 | 11 | 36% | 13 | 101.5 |  |
| rust-sum-inclusive | fix | passed | 是 | 6 | 67% | 5 | 35.7 |  |
| rust-version-display | feature | failed | 是 | 2 | 100% | 1 | 58.7 |  |

状态：passed = 外部验收通过且受保护文件未动；cheated = 验收通过但动了受保护文件；timeout = 超时；skipped = 缺可执行文件没跑；error = 宿主或 Provider 出错没跑成，不进分母。
