# 模型行为评测 · glm-5

跑于 2026-10-14T19:44:52Z · 代码 `bd436f0`（工作区不干净） · willdeep 0.101.0-rc6

| 指标 | 值 |
|---|---|
| 任务 | 20（执行 20 · 跳过 0 · 出错 0） |
| **verifier 通过率** | 100.0%（20/20） |
| 作弊 / 超时 | 0 / 0 |
| 误报完成率 | 0.0% |
| **人话率** | 68.1% |
| 思维链占比 | 72.0% |
| 模型调用 / 静默工具轮 | 182 / 58 |
| 工具调用 / 失败 | 150 / 0 |
| token / 秒 | 1762834 / 859.1 |

| 类型 | 执行 | 通过 | 通过率 |
|---|---:|---:|---|
| fix | 8 | 8 | 100.0% |
| feature | 6 | 6 | 100.0% |
| test | 4 | 4 | 100.0% |
| lint | 2 | 2 | 100.0% |

| 任务 | 类型 | 状态 | 声称完成 | 调用 | 人话率 | 工具调用 | 秒 | 备注 |
|---|---|---|---|---:|---|---:|---:|---|
| js-add-test-clamp | test | passed | 否 | 9 | 78% | 6 | 36.5 | 退出码 5；unverified；变异抓住 2/2 |
| js-flatten-depth | feature | passed | 否 | 10 | 60% | 7 | 32.5 | 退出码 5；unverified |
| js-parse-query-decode | fix | passed | 否 | 17 | 41% | 13 | 42.2 | 退出码 5；unverified |
| python-add-test-normalize | test | passed | 是 | 9 | 67% | 9 | 63.1 | 变异抓住 2/2 |
| python-backoff-delays | feature | passed | 是 | 8 | 88% | 9 | 60.5 |  |
| python-mutable-default | fix | passed | 是 | 4 | 100% | 4 | 28.4 |  |
| ruby-add-test-leap-year | test | passed | 否 | 9 | 78% | 6 | 28.3 | 退出码 5；unverified；变异抓住 2/2 |
| ruby-cart-nil-quantity | fix | passed | 否 | 18 | 44% | 16 | 100.9 | 退出码 5；unverified |
| ruby-csv-quoted-comma | fix | passed | 否 | 9 | 89% | 6 | 34.0 | 退出码 5；unverified |
| ruby-slug | feature | passed | 否 | 9 | 67% | 6 | 25.6 | 退出码 5；unverified |
| rust-add-test-median | test | passed | 是 | 6 | 50% | 5 | 25.8 | 变异抓住 2/2 |
| rust-checked-factorial | fix | passed | 是 | 8 | 62% | 7 | 27.0 |  |
| rust-clippy-clean | lint | passed | 否 | 17 | 53% | 14 | 62.1 | 退出码 5；unverified |
| rust-dedup-order | feature | passed | 是 | 9 | 44% | 9 | 82.2 |  |
| rust-deny-unused | lint | passed | 是 | 6 | 83% | 6 | 25.2 |  |
| rust-last-n-panics | fix | passed | 是 | 6 | 100% | 5 | 30.7 |  |
| rust-parse-duration | feature | passed | 是 | 7 | 86% | 6 | 61.2 |  |
| rust-parse-port-result | fix | passed | 是 | 8 | 88% | 6 | 33.9 |  |
| rust-sum-inclusive | fix | passed | 是 | 6 | 100% | 4 | 25.3 |  |
| rust-version-display | feature | passed | 是 | 7 | 100% | 6 | 33.7 |  |

状态：passed = 外部验收通过且受保护文件未动；cheated = 验收通过但动了受保护文件；timeout = 超时；skipped = 缺可执行文件没跑；error = 宿主或 Provider 出错没跑成，不进分母。
