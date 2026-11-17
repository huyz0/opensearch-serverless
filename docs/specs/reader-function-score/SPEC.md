# Reader: function_score

## Serves

Parity task R10.1 (queries), the `function_score` query; invariant I-13 (REST matches OpenSearch's recordings); ADR 0004 (bounded memory).

## Change

`oss-reader` serves `function_score` at OpenSearch 3.10.0, from `FunctionScoreQueryBuilder.fromXContent` and `doToQuery`, `FieldValueFactorFunctionBuilder` and `DecayFunctionBuilder` / `DecayFunctionParser` (`crates/reader/src/dsl/queries/function_score.rs` parses the body, `function_score_build.rs` builds the query). The query runs on the engine's function-score node (`lucene_engine::function_score::OpenSearchFunctionScore`), which holds OpenSearch's own `float`/`double` arithmetic for the functions, the combining of their scores and `boost_mode`.

**The light build.** The node lives in `lucene-engine`, which also holds the analysis modules (Japanese, Korean, Chinese, Polish, Ukrainian, phonetic dictionaries). The reader depends on it with `default-features = false`, so none of those compile in; what remains is the engine's code without its dictionaries. The reader needs none of them, and `lucene-engine` stays at the pinned revision with every other `lucene-*` crate.

- Keys: `query` (match-all when absent), `boost`, `score_mode` (`multiply`, `first`, `avg`, `max`, `sum`, `min`), `boost_mode` (`multiply`, `replace`, `sum`, `avg`, `min`, `max`), `max_boost`, and either one function inline or `functions`, whose entries take `filter`, `weight` and one function.
- Functions: a bare `weight`; `field_value_factor` (`field`, `factor`, `modifier`, `missing`) on integer, long and date fields; `gauss`, `exp` and `linear` (`origin`, `scale`, `offset`, `decay`, and `multi_value_mode` beside the field) on integer, long and date fields. A date's `scale` and `offset` are time values (`10d`, `2h`, `500ms`, `30s`, `5m`, `nanos`, `micros`), its `origin` a date or epoch milliseconds.
- No functions is the `query` alone. A function with a `filter` applies where the filter matches; a document no function matches keeps the query's score. A `weight` with a function multiplies it.
- Errors as OpenSearch gives them: the parse messages (`already found function`, `you can either define [functions] array or a single function`, `an entry in functions list is missing a function`, unknown names, enum names), `[weight] cannot be negative`, and at the shard the decay's `both [scale] and [origin] must be set`, `parameter [x] not supported!`, `decay must be in the range [0..1]`, `scale must be > 0.0`, `offset must be > 0.0`, a field without `missing`, and a text or keyword field in `field_value_factor`.
- A negative or non-finite final score fails the search (`function score query returned an invalid score`, 500) as it does in OpenSearch; a negative `field_value_factor` result fails it as an `exception` (500), where OpenSearch answers `illegal_argument_exception` (400).

The sub-query's norms are read for a query wrapped in a `function_score` (`search.rs` walks into it for the fields to load), so its scores are Lucene's. The shard failure's `caused_by` is the innermost OpenSearch exception when a `query_shard_exception` wraps one (`failure.rs`), as OpenSearch writes it.

Not in this change, refused as not supported yet: `random_score`, `script_score`, `min_score`, `_name`, a negative `boost` (OpenSearch's message prints the query), a decay on a geo point, on a text or keyword field (OpenSearch's message holds a Java object hash) or on several fields, a date decay without an `origin` (it would be `now`), a fractional or non-numeric time value.

## Acceptance criteria

- AC-1: every `_search` step of `fixtures/compat/search/match.json` gets the status and body OpenSearch 3.10.0 gave, replayed against a reader node over the same blobs. The 75 new steps (named `fs ...`) cover weights, each modifier and decay, `missing`, filters, every `score_mode` and `boost_mode`, `max_boost`, a match-all and a bool query, dates, and the errors. A step whose message depends on the order of the body's keys is not in the replay (it re-sorts keys); `function_score_tests.rs` holds it on the bytes sent.
- AC-2: the parser reads every key and its default and fails as OpenSearch fails.
- AC-3: the build gives the node the settings and the processed scale OpenSearch computes, and fails as the shard fails.
- AC-4: what is not served is refused with one message.

## Test plan

- AC-1: the replay (`crates/compat/tests/node_search.rs`).
- AC-2 and AC-4: `dsl/queries/function_score_tests.rs`.
- AC-3: `dsl/queries/function_score_build_tests.rs`.

## S3 request budget

The query changes only what is scored in the segments the search already reads, and loads the norms of the wrapped query's fields as any scored query does. LIST 0, HEAD 0, PUT 0, no new GET. Idle: 0.

## Memory

The body is held once (at most its size, which the HTTP edge bounds); one function and one filter query is built an entry, at most the body's entries.
