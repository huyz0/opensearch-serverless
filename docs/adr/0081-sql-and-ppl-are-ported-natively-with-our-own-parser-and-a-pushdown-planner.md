# 0081: SQL and PPL are ported natively, with our own parsers, one logical plan and a planner that pushes down into the search path

**Status:** Accepted after independent review, 2026-11-28. One round, accept with changes: five blocking findings (per-name authorisation, bounded pushed aggregations, per-statement scan and row budgets, index-pattern caps, cursor identity and key source) are written into sections 3 and 4.

## Context

The owner decided on 2026-11-28 that every plugin is native, with no Java in the runtime path; Java may stay as a test oracle and as a performance and memory comparison ([java-dependencies](../java-dependencies.md), "SQL and PPL: Not served"). The SQL plugin serves two languages over one endpoint family: `POST /_plugins/_sql` (and the legacy `/_opendistro/_sql`) with `_explain`, `close` for cursors and `settings`, and `POST /_plugins/_ppl` with `_explain`. Both turn a statement into a plan, push the parts OpenSearch can do into a search request (query, sort, size, `_source`, aggregations, scripts), and run the rest in memory.

What exists to build on:

- The coordinator's search path (`oss-coordinator::search`) already fans a request out to shards, reduces hits and aggregations and writes the response; the reader already has terms, composite, histogram, metric and pipeline aggregations, `search_after`, point in time, Painless scripts and the full query DSL.
- `lucene-sql` (huyz0/lucene-rust, now with a router for four hot statement shapes, a plan cache, a group-by kernel rule and the Arrow export) is read-only ANSI SQL over one single-shard index through DataFusion, with its result in the plugin's JSON shape; the plugin serves it at `/_plugins/accelerated/sql` beside the stock endpoint. The table is declared by hand, it reads the segments of one reader, runs on tokio, and has no PPL or OpenSearch dialect. Its Arrow source hands record batches to pyarrow, Polars, DuckDB and Spark over the Arrow C stream.
- lucene-rust's `opensearch-plugin/e2e/bench_stock_sql.py` already runs the same statements against a stock node and compares the statement-to-DSL translation (`_explain`) with the native one. The stock plugin's `_explain` is the oracle for the planner's output.
- [ADR 0080](0080-one-plugin-system-three-tiers.md) gives a first-party port a place to register (a linked-in `NativeInstance` with a manifest, routes and settings) and says a plugin's data access is a host capability, not a crate dependency.

Forces, from the owner's principles:

- **S3 requests are the bill.** A statement must cost the requests of the searches it pushes and no more. Nothing may issue a search per row.
- **Memory is reserved before it is held** ([ADR 0004](0004-bounded-memory-and-streaming.md)). A SQL engine's habit of materialising a join or a sort is exactly what the principle forbids.
- **Nodes own nothing, no lock or lease.** A cursor cannot be state on a node.
- **Scale:** about 10M tenants. A statement is planned per request; no per-index or per-tenant structure exists to be kept warm.
- Stock's behaviour is the specification: statement dialect, response shapes, error texts and settings, because Dashboards (Query Workbench, Discover with PPL, Observability) and the JDBC and ODBC drivers talk to it.

## Decision

### 1. Our own lexers and parsers, one for each language, in a pure crate

SQL and PPL are parsed by hand-written lexers and recursive-descent parsers (Pratt for expressions) in a new crate `oss-query`, layer 1 (siblings of `oss-rest`), depending only on `oss-types` and `oss-mem`. It does no I/O and has no async runtime. The dialects are the plugin's, not ANSI: backtick identifiers, `match`, `match_phrase`, `multi_match`, `query_string`, `score()`, `highlight()`, `nested()`, `SHOW TABLES LIKE`, `DESCRIBE TABLES`, `MATCH_PHRASE_PREFIX`, the plugin's date and text function set, and for PPL the command pipeline (`source`, `where`, `fields`, `stats`, `eval`, `sort`, `head`, `dedup`, `rename`, `top`, `rare`, `parse`, `grok`, `patterns`, `join`, `lookup` and the rest the docs list, in the order the spec fixes). Every parser bounds its input (statement bytes, token count, nesting depth, identifier length) before it allocates, refuses anything past the bound with the plugin's `SyntaxCheckException` shape, and cannot panic on any byte string (a fuzz test is a criterion).

**Rejected: DataFusion, `lucene-sql` or Calcite as the front end.** None parses the dialect: DataFusion is ANSI with no PPL, so the dialect would be rewritten into ANSI before parsing, which reproduces the grammar twice. DataFusion needs tokio, which the node does not have, and its memory pool is advisory rather than a reservation in ours. Its scans read the segments of one reader; ours must go through the coordinator's fan-out, forwarding and authorisation. Its sort, aggregate and join operators spill or hold memory that is not reserved from our pools, so they could not be reused even behind our own parser. `lucene-sql` stays what it is (single-shard ANSI SQL and the Arrow export) and is a Rust oracle and a performance comparison for the ANSI subset of SELECT, beside the stock plugin; it is not linked. Its plan shapes (point read, scored top-k, nearest neighbours, group-by on a keyword) are the first shapes the planner's benchmarks compare against.

### 2. One logical plan for both languages

Both parsers lower to one tree: `Scan` (index expression, pushed filter, sort, size, fields, aggregations) under `Filter`, `Project`, `Eval`, `Aggregate`, `Window`, `Sort`, `Limit`, `Dedup`, `Join` and `Union`. The planner's rules (constant folding, predicate and projection pushdown, `LIMIT` into `size` and the sort's `top_n`, aggregation lowering) work on this tree, so a rule is written once for both languages. The logical tree is a Rust enum with bounded depth and node count (default 256 nodes, a setting).

### 3. Pushdown is the plan; the executor is small and bounded

A planned statement is a list of **scans** and the in-memory operators above them. A scan lowers to a search request (the DSL the existing parsers accept) and runs through the coordinator, so shard fan-out, hit merge, aggregation reduce, resolution of aliases and patterns, routing, request-cost accounting and request-cost accounting are the search path's own and are not reimplemented. Rules:

- **Authorisation is checked here, per name.** [ADR 0051](0051-authorization-at-the-route-by-action-table.md) decides at the route from the URL, and SQL and PPL put index names in the body (`FROM a, b`, patterns, joins, subqueries, `SHOW TABLES LIKE`, `DESCRIBE`). So `Source` checks `indices:data/read/search` (and the mapping and `SHOW` actions) for **every name the planner extracts, as written and before any resolution, before the first scan**; a refusal is the 403 `security_exception` shape and no scan runs. The route has its own action-table row. A test runs a join and a subquery over two indices with a role limited to one.
- **A statement has total budgets.** At most `max_scans` scans (default 8) across joins, subqueries and `UNION`, at most `max_resolved_indices` and `max_resolved_shards` per statement (stock's pattern fan-out is the search path's own; the cap makes it a stated bound), and one total row and byte bound across all in-memory sides. Past any of them is 400 with the plugin's text. A scan pages at most `ceil(size_limit / page)` times.
- **Pushed aggregations are bounded.** `GROUP BY`, `stats by`, `top`, `rare` and `distinct` lower to terms or composite aggregations only under the total-bucket bound of [ADR 0022](0022-nested-bucket-aggregations.md) and a `QueryExecution` reservation; past it is 429 or 400, never an unbounded paging loop.
- **The planner refuses what it cannot push** (the 400 rule below) from the first slice, and no later slice weakens it.

- **Plan shape equals stock's.** A filter, sort, aggregation or relevance function stock pushes is pushed to the same DSL, checked against stock's `_explain` on a recorded corpus. An expression with no DSL form is pushed as a Painless script only where stock does (script filter, script field, script aggregation), through the existing Painless front end ([ADR 0069](0069-painless-source-compiles-in-rust-with-java-classes-as-functions.md)). **Until that slice lands, such a statement is refused 400 with the plugin's text; it is never answered by scanning the index and filtering in memory**, because that would make a request count scale with documents.
- **The in-memory operators are the ones stock runs in memory:** the projection and `eval` over a page, post-aggregation sort and limit, `HAVING` over buckets, window functions, `join`, `union`, `dedup`, `parse` and `grok`. `dedup` and window functions are fed only by a scan already capped by `size_limit`; that cap changes the answer against stock on a larger input and is recorded in [compatibility](../compatibility.md) as a documented difference. Each operator reserves its working set from `Pool::QueryExecution` before it allocates, in a fixed unit; a reservation that fails is 429 `circuit_breaking_exception`-shaped with the plugin's text, never an out-of-memory. No operator spills.
- **A join is a hash join of two bounded scans**, never a per-row lookup: each side is one search (paged by `search_after`), the build side is the smaller declared by the plan, and each side's rows are bounded by `plugins.query.size_limit` (stock default; a join stock would answer by a nested loop is bounded the same way, and the bound is the answer to why it differs). **A statement issues a number of searches fixed by its plan shape and its pages, never by its rows.**
- **Arrow is an output format, not an engine.** A later slice may add `format=arrow` (the Arrow IPC stream, written batch by batch from the same bounded rows, so memory stays bounded); it needs no table declaration because the plan already knows the columns and types. Reading a Rust-store index as an Arrow table for Spark or Polars is a separate feature, not part of this ADR.
- **Result size** is `plugins.query.size_limit` and the response is written to the HTTP body as it is produced, in the format the request names (`jdbc`, `csv`, `raw`, `json`, and `viz` for PPL) without holding the whole result; `fetch_size` pages through a cursor.

### 4. Cursors are self-contained

A cursor carries no node state. It is a bounded token (at most 4 KiB) holding the planned statement's hash, the caller's identity, the resolved index set, an expiry time, and the page position, with an HMAC under the cluster's transport secret of [ADR 0016](0016-native-transport-and-early-revoke.md) (the secrets register; a token is accepted under the current and the previous generation, so a rotation does not strand cursors for more than one window). A node that did not make it can resume it; a forged, expired or other caller's token is refused (400 with the plugin's text, or 403). Authorisation is checked on **every page**, not only the first. By default a page is `search_after` on a sort ending in a tiebreaker (`_shard_doc`, else `_id`), which pins nothing and costs one search per page; a consistent read through a point in time is opt-in, because a capture costs a request per shard of the index set and pins files up to the point-in-time keep-alive ceiling. `close` of a cursor with a point in time releases it through the existing store (one release per shard set); one without makes no request. No timer holds a cursor.

### 5. Mounting, switch and settings

SQL and PPL register as first-party plugin `query` in the plugin system's registry as a linked-in `NativeInstance` once the `search` capability of [ADR 0080](0080-one-plugin-system-three-tiers.md) exists. The only door `oss-query` has to data is one trait, `Source` (resolve names to mappings, run a search request, release a point in time); the node implements it over the coordinator today, and the capability implements it later, so moving onto the plugin system changes no planner code. Until the capability lands the routes are mounted by `oss-node` as the LTR routes are.

The feature is behind **`serverless.sql.enabled`** (`OSS_SQL`), default off until the stock-recorded corpus passes for the slice's scope and the comparison below shows no downside; then on, and the switch is removed once its default is proven ([principles](../principles.md#switches-and-defaults)). With the switch off the routes answer the plugin's disabled-plugin error shape, and idle cost is zero (no timer, no startup read). Stock's own settings are kept under their names for clients that read them (`plugins.sql.enabled`, `plugins.ppl.enabled`, `plugins.query.size_limit`, `plugins.sql.cursor.keep_alive`, `plugins.query.memory_limit` taken as a share of `Pool::QueryExecution`, never as extra memory) and are served by `_plugins/_query/settings`.

### 6. Scope kept out

Not here, and each is a 400 naming the feature: DML and DDL (`DELETE`, `INSERT`, `CREATE`; the plugin's legacy `DELETE` is off by default and stays refused), the data sources API and its external sources (Prometheus, S3 tables, Spark; they are outbound calls to user URLs and need their own ADR on the ML client's rules), `_plugins/_query/_datasources`, and the Calcite-only additions until a slice names them. ODBC and JDBC need no work: they use the plugin's wire shape.

### 7. Order of work

1. Spec, the response writers and the plan IR, with `SELECT` of columns from one index (`WHERE` on term, range, `match`, `AND`/`OR`/`NOT`, `ORDER BY`, `LIMIT`), `jdbc` and `csv`/`raw` output, `_explain`, `SHOW`/`DESCRIBE`.
2. Aggregations (`GROUP BY`, `HAVING`, the aggregate functions, `date_histogram` forms) lowered to the existing aggregation DSL.
3. PPL: the command pipeline over the same plan.
4. Cursors and `fetch_size`; point in time.
5. Function library breadth and script pushdown.
6. Join, window, union, subqueries; the `QueryExecution` bounds measured.
7. Plugin-system adoption; switch flipped on the evidence.

Each slice is one PR, behind the switch, with a spec criterion and a stock-recorded corpus (statements, `_explain` and result), or `NOT-RUN` named where a stock node is not available.

## Consequences

- Easier: a statement costs what its pushed searches cost, by construction; Dashboards' Query Workbench, PPL in Discover and Observability's PPL queries, and the JDBC and ODBC drivers can work against a Rust cluster; the parsers have no dependency to audit or to size.
- Harder: two grammars to write and keep in step with stock's releases; a large function library (the plugin has several hundred functions) written from the docs and the plugin's behaviour, not its source, so shapes stay provisional until recorded from stock; the dialect's date and type conversions need one differential corpus per function family.
- Rules out: scan-and-filter fallbacks, spilling operators, node-held cursors, ANSI-only SQL, per-row lookups in a join, and any dependency on a Java process. It also rules out running a statement the plan cannot bound; a user sees a 400 or 429 with a stated bound, not a slow answer.

## Requests, memory and compatibility, stated for the review

- **Requests:** planning and parsing make none. A statement makes the searches of its scans (one per scan per page) through the existing path, plus the mapping reads the search path already makes (shared-mapping cache and descriptor cache). `SHOW TABLES` and `DESCRIBE` read the same derived name listing the cat and indices APIs use (the spec names the function; a LIST is never issued), paged within `size_limit`. A cursor close makes one request only when it holds a point in time. An idle node makes zero.
- **Memory:** the lexer and parser reserve before allocating and are bounded in bytes, tokens and depth; operators reserve from a `Query` pool of the memory budget; the response streams.
- **Scalability:** no state per tenant, index or cursor on a node; the plan and its cache key are per statement; a per-node plan cache, if measured to pay, is bounded and keyed by statement text.
- **Compatibility:** response shapes and error texts follow the plugin's, recorded from stock where a stock node runs and marked provisional where not.
