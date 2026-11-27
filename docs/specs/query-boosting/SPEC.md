# The `boosting` query

## Serves

The `boosting` refusal of [compatibility](../../compatibility.md): a stock-OpenSearch comparison found `{"boosting":{"positive":..,"negative":..,"negative_boost":..}}` answered 400 here.

## Change

Ported from `BoostingQueryBuilder.fromXContent` and `doToQuery` (server/src/main/java/org/opensearch/index/query/) at the OpenSearch tip the Java shell builds on (`fae98a3`).

- A document matches when `positive` matches; its score is the positive score, times `negative_boost` where `negative` matches too (`FunctionScoreQuery.boostByQuery`). The query's own `boost` is applied as for any query.
- `positive`, `negative` and a non-negative `negative_boost` are required, each with OpenSearch's message; any other key fails "[boosting] query does not support [x]".
- `_name` is refused as not supported yet, as for the other compound queries.

## Acceptance criteria

- AC-1: a search with `boosting` returns the documents `positive` matches, with the score scaled by `negative_boost` where `negative` matches.
- AC-2: a missing `positive`, `negative` or `negative_boost`, a negative `negative_boost` and an unknown key fail as the Java fails.

## S3 request budget

None added: the query runs over the segments the search already reads.

## Switch

None: a refusal becomes an answer.

## Tests

`crates/reader/src/dsl/queries/compound_tests.rs`, `build_tests.rs` and `crates/node/tests/it/search_boosting.rs` through HTTP. No stock recording exists for these cases, so the expected parts are derived from the Java named above and are provisional.
