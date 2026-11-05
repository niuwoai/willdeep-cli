# 空闲评测 daemon：只读核对清单

核对日期：2026-11-05。这里只列出审核范围，尚未停止任何进程，也不删除目录或评测证据。

全机一次进程快照有 142 个 WillDeep 进程（其中 40 个处于 I/O 等待），不能据此把全部进程都当作可停止对象。通过进程 stderr 的真实打开文件，识别出 52 个指向评测临时目录的 daemon。39 个能读取任务与子会话元数据，均没有 Runtime 任务（缺失的 tasks.json 依照存储契约表示空任务集），且 assignment 数为 0；另 13 个缺少所需元数据，排除出建议停止范围。

下面 39 个进程属于同一评测根：`model-eval-run-20261105-1976-2qrgcj`，每项对应 `<评测根>/<模型>/<用例>/home/runtime/daemon.log`。短观察期间合计 revision 增加 90；观察含一秒等待及前后顺序读取开销，因此不报告为精确的每秒速率。没有读取 daemon Token、模型配置、任务正文或回复。

| PID | 模型 | 用例 | 观察到的提交数 |
| ---: | --- | --- | ---: |
| 4226 | glm-5 | python-add-test-normalize | 0 |
| 4775 | glm-5 | python-backoff-delays | 3 |
| 5317 | glm-5 | python-mutable-default | 3 |
| 8340 | glm-5 | rust-add-test-median | 2 |
| 9304 | glm-5 | rust-checked-factorial | 1 |
| 10465 | glm-5 | rust-clippy-clean | 4 |
| 11698 | glm-5 | rust-dedup-order | 2 |
| 12761 | glm-5 | rust-deny-unused | 1 |
| 13366 | glm-5 | rust-last-n-panics | 1 |
| 14685 | glm-5 | rust-parse-duration | 4 |
| 15329 | glm-5 | rust-parse-port-result | 3 |
| 16244 | glm-5 | rust-sum-inclusive | 4 |
| 17162 | glm-5 | rust-version-display | 1 |
| 20151 | deepseek-v4-flash | python-add-test-normalize | 1 |
| 20703 | deepseek-v4-flash | python-backoff-delays | 2 |
| 21265 | deepseek-v4-flash | python-mutable-default | 2 |
| 24646 | deepseek-v4-flash | rust-add-test-median | 5 |
| 25779 | deepseek-v4-flash | rust-checked-factorial | 4 |
| 26712 | deepseek-v4-flash | rust-clippy-clean | 2 |
| 28302 | deepseek-v4-flash | rust-dedup-order | 4 |
| 29412 | deepseek-v4-flash | rust-deny-unused | 2 |
| 30191 | deepseek-v4-flash | rust-last-n-panics | 3 |
| 31000 | deepseek-v4-flash | rust-parse-duration | 2 |
| 31849 | deepseek-v4-flash | rust-parse-port-result | 3 |
| 32521 | deepseek-v4-flash | rust-sum-inclusive | 1 |
| 33510 | deepseek-v4-flash | rust-version-display | 0 |
| 36163 | deepseek-v4-pro | python-add-test-normalize | 4 |
| 36789 | deepseek-v4-pro | python-backoff-delays | 0 |
| 37424 | deepseek-v4-pro | python-mutable-default | 3 |
| 41456 | deepseek-v4-pro | rust-add-test-median | 4 |
| 43153 | deepseek-v4-pro | rust-checked-factorial | 2 |
| 45038 | deepseek-v4-pro | rust-clippy-clean | 4 |
| 46469 | deepseek-v4-pro | rust-dedup-order | 4 |
| 47793 | deepseek-v4-pro | rust-deny-unused | 3 |
| 48562 | deepseek-v4-pro | rust-last-n-panics | 2 |
| 49621 | deepseek-v4-pro | rust-parse-duration | 0 |
| 50937 | deepseek-v4-pro | rust-parse-port-result | 0 |
| 52031 | deepseek-v4-pro | rust-sum-inclusive | 2 |
| 53017 | deepseek-v4-pro | rust-version-display | 2 |

若获准停止，执行前必须重新核对进程身份、打开的评测目录、Runtime 任务/子会话状态与评测是否仍被其他工作使用；PID 可能复用，不可直接照本表批量 kill。应通过对应隔离数据目录的正常 daemon stop 入口逐个优雅停止，保留全部文件；日用 daemon、其他评测根和未确认的 13 个进程不在范围内。停止后再复测首答与内容快照，才能评估竞争减少后的实际收益。
