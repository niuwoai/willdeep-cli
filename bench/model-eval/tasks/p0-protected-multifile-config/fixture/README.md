# Export fixture

`Manifest.build(items, config:)` returns a JSON-compatible hash with `schema_version`,
`total_items`, and `batches`. Each batch contains its zero-based `index` and `items`.
`Batching.chunks(items, limit:)` preserves order and the final partial batch.
Neither operation mutates its arguments. The configured limit must be a positive
Integer. Config keys are mandatory. These files and config/export.json are protected;
only lib/batching.rb and lib/manifest.rb may change.

Run `ruby test/contract_test.rb`. No gems or network access are needed.
