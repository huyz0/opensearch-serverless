# PPL, slice 1: the pipe language over the SQL planner

## Serves

[ADR 0082](../../adr/0082-sql-and-ppl-are-ported-natively-with-our-own-parser-and-a-pushdown-planner.md), order of work item 3. Builds on [sql-select](../sql-select/SPEC.md) and [sql-aggregations](../sql-aggregations/SPEC.md). The reference is the PPL part of the recorded corpus in `fixtures/sql` (stock 3.10.0, 25 statements `ppl-*`).

## Change

`oss-query` gains a PPL lexer and parser. A PPL statement parses to the same `Select` the SQL parser gives, so one planner, one lowering and one set of writers serve both languages (ADR 0082). The commands of this slice:

- `source=<name>[,<name>...]` (also `search source=...`), a prefix pattern like SQL's `FROM`.
- `where <condition>`: `=`, `!=`, `<`, `<=`, `>`, `>=`, `and`, `or`, `not`, `in (...)`, `like(field, 'p%')`, `isnull(f)`, `isnotnull(f)`, and the relevance functions SQL supports.
- `fields [+] a, b` and `fields - a, b`; `sort [+|-] f, ...`; `head [n] [from m]`; `rename a as b[, ...]`.
- `stats <agg>[as alias], ... [by f, ...]` with `count()`, `count(f)`, `sum`, `avg`, `min`, `max`; `top [n] f` and `rare [n] f` as the `stats count() by f` they are, ordered and cut.
- Not in this slice: `eval`, `parse`, `dedup`, `join`, `lookup`, `trendline`, `eventstats`, `streamstats`, `fillnull`, `expand`, `flatten`, `patterns`, `ml`, `kmeans`, `describe`, `show datasources`. Each is a 400 with the plugin's `SyntaxCheckException` shape for an unknown command or the `UnsupportedOperationException` shape for a known one.

Rows come out in PPL's vocabulary: the schema types are `string`, `int`, `bigint`, `double`, `boolean`, `timestamp`, `ip`, `struct`, not SQL's `keyword`, `integer`, `long`. `source=x` with no `fields` lists the top-level fields in the plugin's order (the Java `HashMap` order SQL's `SELECT *` has) with an object as one `struct` column; `fields - a` lists the remaining flat leaves. A `stats` result lists the aggregates first, then the group fields (`c, avg(age), city`).

A sort after `stats` (`stats count() as c by city | sort - c`) orders the bucket rows in memory: bounded by `serverless.sql.max_buckets`, never the documents. The same rule serves SQL's `ORDER BY` an aggregate, which slice 2 refused.

## Acceptance criteria

- AC-1: over the recorded documents each `ppl-*` statement above gives stock's `datarows` and `schema` (names and PPL types), compared by `crates/query/tests/recorded.rs` with the SQL cases; a statement outside this slice is on its `LATER` list and a listed statement that starts working fails the test.
- AC-2: a PPL statement and the SQL statement it equals lower to the same search body (`where age > 26 | fields name, age` and `SELECT name, age FROM t WHERE age > 26`); no PPL-only lowering path exists.
- AC-3: errors: an unknown command is 400 `SyntaxCheckException` with reason `Invalid Query`; a syntax error is 400 `SyntaxCheckException` with the offending token and the position text `[<token>] is not a valid term at this part of the query: '...<tail>' <-- HERE.` (the list of expected tokens that follows in stock is not reproduced: it names 453 grammar tokens); an unknown field is 400 `IllegalArgumentException` `Field [nope] not found.`; an unknown index is 404 `IndexNotFoundException`.
- AC-4: every parser bound of SQL holds (statement bytes, tokens, nesting, identifier length), a fixed-seed fuzz mutates the PPL corpus and no input panics.
- AC-5: no body holds a `script` key; a command that would need one is a 400.
- AC-6: routes `/_plugins/_ppl`, `/_opendistro/_ppl` and their `/_explain` forms, GET and POST, `format` of `jdbc` (default), `csv`, `raw`; with `serverless.ppl.enabled` false every route answers the plugin's disabled error and makes no store request.
- AC-7: a PPL statement costs the requests of the SQL statement it equals (AC-2): one search a page, nothing else; counted with the in-process meter beside the SQL count.
- AC-8: the names of a statement are permitted before any search, exactly as SQL's (the route row is `Rule::Body`); a statement naming a name the caller may not read is 403 and runs no search.

## S3 request budget

Equal to SQL's: one search per composite page for `stats ... by`, one search otherwise; parsing, `_explain` and refusals cost none. A budget row per route when the node test lands.

## Crate

`oss-query` gains `ppl.rs` (lexer is shared, `ppl_parser.rs` for the commands) and a `Dialect` the writers read for type names. `oss-node` mounts four routes on the SQL service (a `language` argument), adds `serverless.ppl.enabled` (`OSS_PPL`) and its authorisation rows. No new crate.

## Switch

`serverless.ppl.enabled` (`OSS_PPL`), default on, as SQL's (owner decision for plugin surfaces: no idle cost, no downside measured). With it false the routes answer the plugin's disabled error.

## Risks

- Stock 3.10 runs PPL through its Calcite engine, so its `_explain` is a Calcite plan and gives no search body to compare: this slice is held by rows and schema, not bodies (AC-1, AC-2). The recorded SQL bodies are the oracle for the lowering both languages share.
- The syntax error text lists expected tokens from stock's grammar; ours stops at the position, a documented difference.
- `top`/`rare` in stock use a window function; here they are the grouped count they equal, cut at `size_limit`: the same rows for `top n` over the groups, bounded by `max_buckets`.
