# 模型行为评测 · deepseek-v4-flash

跑于 2026-11-06T20:15:16Z · 代码 `2a2fe3b8`（工作区不干净） · willdeep 0.147.0-rc3

| 指标 | 值 |
|---|---|
| 任务 | 20（执行 20 · 跳过 0 · 出错 0） |
| **verifier 通过率** | 100.0%（20/20） |
| 作弊 / 超时 | 0 / 0 |
| 误报完成率 | 0.0% |
| **人话率** | 41.2% |
| 思维链占比 | 42.3% |
| 模型调用 / 静默工具轮 | 194 / 114 |
| 工具调用 / 失败 | 181 / 0 |
| token / 秒 | 2792397 / 926.8 |

| 类型 | 执行 | 通过 | 通过率 |
|---|---:|---:|---|
| fix | 8 | 8 | 100.0% |
| feature | 6 | 6 | 100.0% |
| test | 4 | 4 | 100.0% |
| lint | 2 | 2 | 100.0% |

| 任务 | 类型 | 状态 | 声称完成 | 调用 | 人话率 | 工具调用 | 秒 | 备注 |
|---|---|---|---|---:|---|---:|---:|---|
| js-add-test-clamp | test | passed | 否 | 11 | 46% | 9 | 42.9 | 退出码 5；unverified；变异抓住 2/2 |
| js-flatten-depth | feature | passed | 否 | 13 | 38% | 11 | 51.4 | 退出码 5；unverified |
| js-parse-query-decode | fix | passed | 否 | 14 | 36% | 11 | 45.7 | 退出码 5；unverified |
| python-add-test-normalize | test | passed | 是 | 4 | 50% | 4 | 20.8 | 变异抓住 2/2 |
| python-backoff-delays | feature | passed | 是 | 4 | 50% | 4 | 22.1 |  |
| python-mutable-default | fix | passed | 是 | 4 | 50% | 4 | 18.1 |  |
| ruby-add-test-leap-year | test | passed | 否 | 13 | 46% | 12 | 45.0 | 退出码 5；unverified；变异抓住 2/2 |
| ruby-cart-nil-quantity | fix | passed | 否 | 14 | 36% | 13 | 52.0 | 退出码 5；unverified |
| ruby-csv-quoted-comma | fix | passed | 否 | 14 | 36% | 12 | 51.2 | 退出码 5；unverified |
| ruby-slug | feature | passed | 否 | 11 | 64% | 9 | 39.9 | 退出码 5；unverified |
| rust-add-test-median | test | passed | 是 | 7 | 43% | 6 | 36.8 | 变异抓住 2/2 |
| rust-checked-factorial | fix | passed | 是 | 7 | 57% | 7 | 38.1 |  |
| rust-clippy-clean | lint | passed | 否 | 24 | 42% | 26 | 147.3 | 退出码 5；unverified |
| rust-dedup-order | feature | passed | 是 | 7 | 14% | 7 | 43.2 |  |
| rust-deny-unused | lint | passed | 是 | 9 | 33% | 10 | 52.6 |  |
| rust-last-n-panics | fix | passed | 是 | 7 | 43% | 6 | 35.9 |  |
| rust-parse-duration | feature | passed | 是 | 9 | 33% | 9 | 59.1 |  |
| rust-parse-port-result | fix | passed | 是 | 8 | 38% | 8 | 43.6 |  |
| rust-sum-inclusive | fix | passed | 是 | 8 | 38% | 8 | 50.4 |  |
| rust-version-display | feature | passed | 是 | 6 | 50% | 5 | 30.7 |  |

状态：passed = 外部验收通过且受保护文件未动；cheated = 验收通过但动了受保护文件；timeout = 超时；skipped = 缺可执行文件没跑；error = 宿主或 Provider 出错没跑成，不进分母。
