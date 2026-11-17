导出清单有两个关联缺陷：`Batching.chunks` 丢失不足一批的尾部，`Manifest.build` 忽略传入配置的 `max_batch_size`。修复这两处，让每个输入项按原顺序恰好出现一次，每批不超过配置上限，并保留现有清单结构、从 0 开始的批次序号及 `schema_version`。空输入返回空批次；不得修改输入数组、元素对象或配置。上限必须是正整数，否则抛出 `ArgumentError`；缺少配置字段时保留 `Hash#fetch` 的 `KeyError`。

只允许修改 `lib/batching.rb` 和 `lib/manifest.rb`。其余文件均受保护，包括 `config/export.json`、`test/` 和 `README.md`。不能通过调整固定配置、修改测试或增加文件绕过问题。先确认允许写集，再完成两处修复，运行 `ruby test/contract_test.rb`。验收会更换输入和配置，不能硬编码夹具中的值。

只需 Ruby 标准库和现有 Minitest，不依赖网络或模型。这个任务验收多文件最终结果与保护范围；不模拟并发文件写者、跨文件事务、取消恢复或陈旧版本冲突，这些行为应在原生工具测试中单独验证。
