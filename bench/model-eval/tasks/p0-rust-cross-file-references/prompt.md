把 `pricing::quote` API 迁移为 `pricing::quote_cents`，移除旧名称，并更新本工作区中属于该函数的全部引用（包括导入别名）。计算规则保持不变：单价乘数量，再减去按基点计算并向下取整的折扣；折扣上限为 10,000 基点。

只允许修改 `src/pricing.rs`、`src/invoice.rs`、`src/checkout.rs`。其余文件均受保护，尤其是 `src/shipping.rs` 中独立的 `quote`、字符串中的 `quote`、`Cargo.toml` 和 `test/`。不要为通过测试修改这些文件或增加构建脚本。先定位定义与引用，再确认允许写集，并运行 `ruby test/verify.rb`。

夹具没有外部依赖。宿主验收需要 Ruby 和原生 rustc >=1.56（Rust 2021），直接编译临时目录，不调用 Cargo 或网络。验收检查最终 API、行为、跨文件调用和保护范围；它不验证使用了哪一种查询或编辑工具，也不证明编辑版本协议已经正确执行。
