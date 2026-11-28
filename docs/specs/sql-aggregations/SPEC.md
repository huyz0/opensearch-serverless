# SQL, slice 2: aggregations pushed into the search path

## Serves

[ADR 0082](../../adr/0082-sql-and-ppl-are-ported-natively-with-our-own-parser-and-a-pushdown-planner.md), order of work item 2. Builds on [sql-select](../sql-select/SPEC.md). The reference is the recorded corpus in `fixtures/sql` (stock 3.10.0): `count-star`, `count-field`, `count-where`, `sum-avg-min-max`, `group-by`, `group-by-having`, `group-by-order`, `group-by-two`, `distinct`.

## Change

`oss-query` reads `COUNT(*)`, `COUNT(field)`, `SUM`, `AVG`, `MIN`, `MAX` in the select list, `GROUP BY` one or more fields, `HAVING` over an aggregate or its alias, `SELECT DISTINCT` over fields, and `ORDER BY` a group key, and lowers them to one search body with `size: 0` and `aggregations`. Without `GROUP BY` the aggregates are top-level aggregations; with it they are sub-aggregations of one `composite` aggregation. The node pages the composite with `after_key` until it is exhausted or a bound is passed, and the writer turns buckets into rows.

Not in this slice: `COUNT(DISTINCT x)`, `GROUP BY` an expression, `HAVING` that is not a comparison with a literal, window functions, date histogram forms, and aggregates over scripts. Each is a 400 with the plugin's text.

## Acceptance criteria

- AC-1: over the recorded documents, each corpus statement above gives stock's `datarows` and `schema` (names, aliases, types), compared by the same test that holds slice 1 (`crates/query/tests/recorded.rs`), with the statements removed from its `LATER` list.
- AC-2: the body for an ungrouped aggregate holds `size: 0` and one aggregation per aggregate, named as the select item is written (`count(*)`, `sum(age)`), `COUNT(*)` as `value_count` on `_index` and `COUNT(field)` as `value_count` on the field, as stock sends them; the `WHERE` is the same `query` slice 1 lowers.
- AC-3: the body for `GROUP BY` holds one `composite` aggregation named `composite_buckets` with one `terms` source per group field, `missing_bucket: true`, `missing_order: first`, and `order` from `ORDER BY` (ascending by default), and the aggregates as its sub-aggregations. A null group is one row, first.
- AC-4: `HAVING` is applied to the bucket rows by the writer, over the aggregate values only; `HAVING` that names a field outside the select list or the group is a 400.
- AC-5: the result row count is bounded: a composite is paged in pages of `min(1000, remaining bucket budget)`; past `max_buckets` (default 10,000, under the ADR 0022 bucket budget) the statement is a 400 and no further page is requested. The writer keeps only the rows, so memory is the rows, not the buckets.
- AC-6: a statement that groups on a text field with no keyword sub-field, or sums a non-numeric field, is a 400 with the plugin's text; `ORDER BY` an aggregate is a 400 until the next slice.
- AC-7: a grouped statement costs `ceil(buckets / page)` searches over its resolved indices (one when the buckets fit one page) and nothing else; counted with the in-process emulator and the meter against the same page count of equal `_search` calls.
- AC-8: no generated body holds a `script` key; `SELECT DISTINCT` is the same composite with no sub-aggregations.

## S3 request budget

One search per composite page, each exactly an equal `_search`'s. Parsing, `_explain` and refusals: 0. A row in [request-budgets](../../request-budgets.md) per route when the node test lands.

## Crate

No new crate, setting or route: `oss-query` gains `aggregate.rs` (lowering and bucket rows), the node backend gains the page loop in `search_sql.rs`. `max_buckets` is a new node setting under `serverless.sql.` (node scope, default 10,000), so the PR is high risk.

## Switch

Rides `serverless.sql.enabled`. No separate switch: the statements were a 400 before and are rows now.

## Risks

- Composite paging changes the answer when documents change between pages; stock has the same property, so it is not defended here. A point in time stays opt-in (ADR 0082).
- Stock computes `HAVING` in its engine, over the same bucket rows; the cap on buckets is a documented difference from stock, which stops at its own 1,000-per-page composite loop with a larger default budget.
- Stock's `avg` type is double and `sum` follows the field type (`sum(age)` on an integer field is `integer`); the schema types come from the recorded corpus, not from this spec's wording.
