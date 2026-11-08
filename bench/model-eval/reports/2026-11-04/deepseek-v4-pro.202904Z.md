# 模型行为评测 · deepseek-v4-pro

跑于 2026-11-04T20:29:04Z · 代码 `ced9c11b`（工作区不干净） · willdeep 0.143.0-rc7

| 指标 | 值 |
|---|---|
| 任务 | 20（执行 20 · 跳过 0 · 出错 0） |
| **verifier 通过率** | 100.0%（20/20） |
| 作弊 / 超时 | 0 / 0 |
| 误报完成率 | 0.0% |
| **人话率** | 37.1% |
| 思维链占比 | 40.3% |
| 模型调用 / 静默工具轮 | 159 / 100 |
| 工具调用 / 失败 | 147 / 0 |
| token / 秒 | 2106613 / 1293.9 |

| 类型 | 执行 | 通过 | 通过率 |
|---|---:|---:|---|
| fix | 8 | 8 | 100.0% |
| feature | 6 | 6 | 100.0% |
| test | 4 | 4 | 100.0% |
| lint | 2 | 2 | 100.0% |

| 任务 | 类型 | 状态 | 声称完成 | 调用 | 人话率 | 工具调用 | 秒 | 备注 |
|---|---|---|---|---:|---|---:|---:|---|
| js-add-test-clamp | test | passed | 否 | 9 | 44% | 6 | 59.7 | 退出码 5；unverified；变异抓住 2/2 |
| js-flatten-depth | feature | passed | 否 | 11 | 36% | 8 | 63.3 | 退出码 5；unverified |
| js-parse-query-decode | fix | passed | 否 | 11 | 36% | 8 | 65.4 | 退出码 5；unverified |
| python-add-test-normalize | test | passed | 是 | 4 | 50% | 5 | 46.3 | 变异抓住 2/2 |
| python-backoff-delays | feature | passed | 是 | 5 | 40% | 5 | 50.0 |  |
| python-mutable-default | fix | passed | 是 | 4 | 50% | 4 | 38.4 |  |
| ruby-add-test-leap-year | test | passed | 否 | 9 | 44% | 6 | 54.6 | 退出码 5；unverified；变异抓住 2/2 |
| ruby-cart-nil-quantity | fix | passed | 否 | 13 | 38% | 10 | 71.3 | 退出码 5；unverified |
| ruby-csv-quoted-comma | fix | passed | 否 | 9 | 56% | 6 | 51.7 | 退出码 5；unverified |
| ruby-slug | feature | passed | 否 | 9 | 56% | 6 | 57.9 | 退出码 5；unverified |
| rust-add-test-median | test | passed | 是 | 7 | 43% | 6 | 64.5 | 变异抓住 2/2 |
| rust-checked-factorial | fix | passed | 是 | 9 | 11% | 11 | 65.3 |  |
| rust-clippy-clean | lint | passed | 是 | 15 | 33% | 17 | 134.7 |  |
| rust-dedup-order | feature | passed | 是 | 7 | 29% | 6 | 73.9 |  |
| rust-deny-unused | lint | passed | 是 | 8 | 38% | 11 | 70.6 |  |
| rust-last-n-panics | fix | passed | 是 | 5 | 20% | 6 | 58.9 |  |
| rust-parse-duration | feature | passed | 是 | 6 | 33% | 6 | 72.8 |  |
| rust-parse-port-result | fix | passed | 是 | 7 | 29% | 8 | 71.5 |  |
| rust-sum-inclusive | fix | passed | 是 | 5 | 20% | 6 | 56.8 |  |
| rust-version-display | feature | passed | 是 | 6 | 33% | 6 | 66.3 |  |

状态：passed = 外部验收通过且受保护文件未动；cheated = 验收通过但动了受保护文件；timeout = 超时；skipped = 缺可执行文件没跑；error = 宿主或 Provider 出错没跑成，不进分母。
