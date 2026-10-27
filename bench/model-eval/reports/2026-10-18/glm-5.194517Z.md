# 模型行为评测 · glm-5

跑于 2026-10-18T19:45:17Z · 代码 `bd436f0`（工作区不干净） · willdeep 0.104.0-rc4

| 指标 | 值 |
|---|---|
| 任务 | 20（执行 20 · 跳过 0 · 出错 0） |
| **verifier 通过率** | 100.0%（20/20） |
| 作弊 / 超时 | 0 / 0 |
| 误报完成率 | 0.0% |
| **人话率** | 73.0% |
| 思维链占比 | 69.5% |
| 模型调用 / 静默工具轮 | 167 / 45 |
| 工具调用 / 失败 | 144 / 0 |
| token / 秒 | 1621249 / 889.3 |

| 类型 | 执行 | 通过 | 通过率 |
|---|---:|---:|---|
| fix | 8 | 8 | 100.0% |
| feature | 6 | 6 | 100.0% |
| test | 4 | 4 | 100.0% |
| lint | 2 | 2 | 100.0% |

| 任务 | 类型 | 状态 | 声称完成 | 调用 | 人话率 | 工具调用 | 秒 | 备注 |
|---|---|---|---|---:|---|---:|---:|---|
| js-add-test-clamp | test | passed | 否 | 9 | 100% | 6 | 24.2 | 退出码 5；unverified；变异抓住 2/2 |
| js-flatten-depth | feature | passed | 否 | 10 | 70% | 7 | 27.8 | 退出码 5；unverified |
| js-parse-query-decode | fix | passed | 否 | 19 | 42% | 15 | 68.3 | 退出码 5；unverified |
| python-add-test-normalize | test | passed | 是 | 6 | 83% | 7 | 68.1 | 变异抓住 2/2 |
| python-backoff-delays | feature | passed | 是 | 4 | 100% | 4 | 15.7 |  |
| python-mutable-default | fix | passed | 是 | 4 | 100% | 4 | 31.6 |  |
| ruby-add-test-leap-year | test | passed | 否 | 8 | 88% | 5 | 25.1 | 退出码 5；unverified；变异抓住 2/2 |
| ruby-cart-nil-quantity | fix | passed | 否 | 13 | 77% | 10 | 42.6 | 退出码 5；unverified |
| ruby-csv-quoted-comma | fix | passed | 否 | 9 | 67% | 6 | 43.6 | 退出码 5；unverified |
| ruby-slug | feature | passed | 否 | 9 | 78% | 6 | 28.1 | 退出码 5；unverified |
| rust-add-test-median | test | passed | 是 | 8 | 88% | 9 | 66.2 | 变异抓住 2/2 |
| rust-checked-factorial | fix | passed | 是 | 7 | 71% | 6 | 28.3 |  |
| rust-clippy-clean | lint | passed | 否 | 14 | 57% | 13 | 45.9 | 退出码 5；unverified |
| rust-dedup-order | feature | passed | 是 | 6 | 100% | 5 | 28.8 |  |
| rust-deny-unused | lint | passed | 是 | 7 | 86% | 6 | 29.1 |  |
| rust-last-n-panics | fix | passed | 是 | 6 | 33% | 6 | 60.8 |  |
| rust-parse-duration | feature | passed | 是 | 7 | 100% | 6 | 33.4 |  |
| rust-parse-port-result | fix | passed | 是 | 9 | 67% | 11 | 138.6 |  |
| rust-sum-inclusive | fix | passed | 是 | 5 | 60% | 5 | 21.3 |  |
| rust-version-display | feature | passed | 是 | 7 | 71% | 7 | 61.8 |  |

状态：passed = 外部验收通过且受保护文件未动；cheated = 验收通过但动了受保护文件；timeout = 超时；skipped = 缺可执行文件没跑；error = 宿主或 Provider 出错没跑成，不进分母。
