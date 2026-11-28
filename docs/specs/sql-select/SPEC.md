# SQL, slice 1: SELECT over indices, pushed into the search path

## Serves

[ADR 0082](../../adr/0082-sql-and-ppl-are-ported-natively-with-our-own-parser-and-a-pushdown-planner.md), order of work item 1. Follows the shape of [ltr](../ltr/SPEC.md) for mounting. PPL, aggregations, cursors, joins, script pushdown and the plugin-system move are later slices.

## Change

A new pure crate `oss-query` (layer 1: depends on `oss-types` and `oss-mem` only, no I/O, no async runtime) holds the SQL lexer, parser, logical plan, the lowering of a plan to a search request body, and the response writers. Its one door to data is `trait Source` (resolve and authorise names, read a mapping, run a search body, return the response bytes). `oss-node` mounts the routes and implements `Source` over the coordinator's search path. Everything is behind `serverless.sql.enabled` (`OSS_SQL`), default off.

Routes: `POST|GET /_plugins/_sql` and `/_opendistro/_sql` (body `{"query": ..., "fetch_size"?, "filter"?, "parameters"?}`; query parameter `format` = `jdbc` (default), `csv`, `raw`; `json` is a later slice and is a 400 until then), `POST /_plugins/_sql/_explain`, and `/_plugins/_query/settings` GET.

Statements in this slice: `SELECT [DISTINCT-refused] <columns | * | expr AS alias> FROM <index | prefix pattern | alias> [WHERE ...] [ORDER BY ...] [LIMIT n [OFFSET m]]`, `SHOW TABLES LIKE '<prefix>%'`, `DESCRIBE TABLES LIKE '<prefix>%' [COLUMNS LIKE ...]`. `WHERE`: `=, <>, <, <=, >, >=, BETWEEN, IN, IS [NOT] NULL, LIKE`, `AND/OR/NOT`, and the relevance functions `match, match_phrase, match_phrase_prefix, multi_match, query_string, match_bool_prefix` with named options. Select list: columns, `_id`, `_score`, `_index`, `AS`. Backtick and double-quote identifiers, `.`-paths, single-quoted strings, integers, decimals, booleans, `NULL`, `--` and `/* */` comments.

Not in this slice, each a 400 with the plugin's text: `GROUP BY`, aggregate functions, `HAVING`, `JOIN`, subqueries, `UNION`, window functions, scalar functions other than the relevance functions (they need script pushdown), `fetch_size` paging (a `fetch_size` at or below `size_limit` is accepted and the page is the first one; a cursor is not returned), `DELETE`, `INSERT`, `CREATE`, `format=viz`.

## Acceptance criteria

- AC-1: with `serverless.sql.enabled` false, every route answers the plugin's disabled-plugin error shape (400), and the node makes zero store requests and runs no timer for it; with it true the routes answer.
- AC-2: the parser takes any byte string and never panics: statements past `max_statement_bytes` (default 64 KiB), 4,096 tokens, nesting depth 64 or identifier length 255 are refused before allocation proportional to the excess, with the plugin's `SyntaxCheckException` shape; a fuzz test over random and mutated corpus statements runs under a fixed seed and a time bound.
- AC-3: the lowering of each corpus statement equals the DSL stock's `_explain` gives for it (query, sort, size, `from`, `_source`, `track_scores`), compared as JSON values after dropping keys stock adds with defaults; the corpus covers every WHERE form above (inline in `crates/query/tests/lowering.rs`, 40 statements at the first PR; more are added with each form). Where no stock recording exists, the row is `NOT-RUN` in VERIFIED.md and the expected DSL is hand-written from the plugin docs and marked provisional.
- AC-4: a statement the planner cannot push (any construct listed under "not in this slice", or an expression with no DSL form) is refused 400 and **no search runs**, asserted by the meter (zero requests) for every refused construct.
- AC-5: the response in `jdbc` form is `{schema: [{name, alias?, type}], datarows, total, size, status: 200}`, `csv` and `raw` are the plugin's text forms (quoting, header row, `raw` without header), over a fixed corpus of results recorded or hand-written and marked provisional; types map from the mapping (`keyword`, `text`, `long`, `integer`, `double`, `boolean`, `date`, `timestamp` forms, `ip`, `object`, `nested`).
- AC-6: every index name the planner extracts (`FROM`, `SHOW`/`DESCRIBE` patterns) is checked, as written and before any resolution, for `indices:data/read/search` (`indices:admin/mappings/get` for `DESCRIBE`) before the first scan; a role limited to one index running `FROM a, b` gets 403 `security_exception` and no scan runs. `_plugins/_sql` has its row in the action table.
- AC-7: a statement costs the requests of one search over its resolved indices and nothing else: with the in-process emulator and the meter, `SELECT ... FROM one_index WHERE ...` makes exactly the requests an equal `_search` makes (asserted equal to the same body sent through `_search`), including the mapping reads; `_explain` makes zero object-store requests beyond the mapping read, and a parse error makes zero.
- AC-8 (amended for the node mount: the node search path refuses wildcard patterns today, so SQL takes the same names `_search` takes, and a pattern is a 400 until the coordinator resolver is wired; `SHOW TABLES` and `DESCRIBE ... LIKE` are deferred to that step, `DESCRIBE` of one named index works): a name that is a prefix pattern resolves through the coordinator's capped prefix listing (the existing resolver, one marked LIST) and any other wildcard form (`%x%`, `*x*`, a pattern with a middle wildcard) is 400; `SHOW TABLES` and `DESCRIBE` read the same derived listing the `_cat/indices` plan uses and make no other LIST. Past `max_resolved_indices` (default 100) or `max_resolved_shards` (default 1,000) is 400 with the plugin's text and no scan runs.
- AC-9: `LIMIT` is capped at `plugins.query.size_limit` (default 200, as stock; settable up to 10,000); no `LIMIT` means the size limit; an `OFFSET` beyond `index.max_result_window` is 400; the result is written to the body as rows are produced and its reservation from `Pool::QueryExecution` is charged before the first row is held, so a result that does not fit is 429 and nothing is allocated past the pool.
- AC-10: `SHOW TABLES LIKE 'pre%'` and `DESCRIBE TABLES LIKE 'pre%'` give the plugin's column sets (`TABLE_CAT`, `TABLE_NAME`, ..., and the 24-column describe row) for the corpus, in name order, bounded by `size_limit`.
- AC-11: a request body past `max_request_bytes` (default 1 MiB) is 413 before it is read whole; malformed JSON is 400 with the position.
- AC-12: Painless-free filter forms only: no generated search body contains a `script` key in this slice, asserted over the whole corpus.
- AC-13: stock's `plugins.sql.enabled`, `plugins.query.size_limit` and `plugins.query.memory_limit` read and update through `_plugins/_query/settings` with stock's response shape; `memory_limit` is a share of `Pool::QueryExecution` and a value above the pool is 400.

## Test plan

Each test is written first and seen red against a stub that returns 501. Unit tests in `oss-query` cover the lexer, parser, lowering and writers with no I/O (AC-2, 3, 4, 5, 9 writer part, 10, 12); the corpus lives in `fixtures/sql/` as one file per statement with `.sql`, `.dsl.json`, `.jdbc.json`, `.csv`, a header comment saying whether it was recorded or hand-written. The fuzz test (AC-2) mutates the corpus under a fixed seed. Node tests over the in-process server cover AC-1, 6, 7, 8, 9 (pool), 11, 13 with the counting meter; AC-6 plants a role limited to one index and a two-index statement; AC-7 sends the lowered body through `_search` and compares counts. Planted violations (an unchecked name, a script key, a LIST for a non-prefix pattern, an unreserved row buffer) are seen failing, and `scripts/selftest-checks.py` gets a row for any new check.

## S3 request budget

`SELECT` over N resolved indices: exactly an equal `_search`'s (shared-mapping and descriptor cache reads, shard reads). Parsing, `_explain` planning and refusals: 0. A prefix pattern: the resolver's one capped, marked LIST, as `_search` on the same pattern. `SHOW TABLES` and `DESCRIBE`: the `_cat/indices` plan's reads. Idle: 0. A row in [request-budgets](../../request-budgets.md) per route when it lands.

## Crate

`oss-query` is layer 1, depends on `oss-types` and `oss-mem`, never on the reader, writer or coordinator; `oss-node` mounts and implements `Source`. The slice adds its README, `AGENTS.md`, `port-map.tsv` (naming the plugin's grammar and classes it mirrors) and the entries in `architecture.md` and the layering check.

## Switch

`serverless.sql.enabled` (`OSS_SQL`), default on (owner decision: it is a plugin surface and costs nothing idle). Its evidence is still owed, recorded in VERIFIED.md before the slice closes: the stock-recorded corpus for AC-3 and AC-5 or an owner-accepted gap list; the request rows; and the comparison against the stock plugin on the same data (latency within 5%, peak memory not above). Other settings (node scope, registered with defaults): `max_statement_bytes`, `max_request_bytes`, `max_resolved_indices`, `max_resolved_shards`, `max_scans` (reserved for later slices), `size_limit`.

## Risks

- Stock's source is not in this sandbox: the grammar, function set and error texts come from the published docs and from lucene-rust's `bench_stock_sql.py` shapes, so the corpus is provisional until `_explain` and results are recorded from a stock node; AC-3 and AC-5 say so row by row.
- A hand-written parser can diverge from ANTLR's on odd inputs (keywords as identifiers, nested comments); the fuzz test finds panics, not divergences, so the corpus must include the odd forms stock accepts and refuses.
- `size_limit` 200 by default changes nothing for stock users but surprises native-API users; the response says when it stopped short.
