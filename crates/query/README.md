# oss-query

SQL (and later PPL) for the serverless node: lexers and parsers for the plugin's dialects, the logical plan, its lowering to a search request body, and the response writers (`jdbc`, `csv`, `raw`, `json`).

## Owns
The statement grammar, the plan, the lowering to a search body and the result formats.

## Depends on
`serde_json` only. No other workspace crate; the node mounts it and supplies names, mappings and search answers.

## Entry points
`src/lib.rs` re-exports them:

- `parse(sql, &Limits)` gives a `Statement` (`Select`, `ShowTables`, `DescribeTables`) or an `Error` in the plugin's shape.
- `plan(&Select, &Mapping, size_limit)` gives a `Plan`: the search body and the output columns.
- `rows(&Plan, &response)` reads the rows out of a search answer; `write(Format, &Plan, &rows, max_bytes)` writes them.
