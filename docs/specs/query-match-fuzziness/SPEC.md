# `fuzziness` on a `match` query

## Serves

The `[match] [fuzziness]` refusal of [compatibility](../../compatibility.md): a stock-OpenSearch comparison found `{"match":{"title":{"query":"quikc","fuzziness":"AUTO"}}}` answered 400 here.

## Change

Ported from `MatchQueryBuilder.fromXContent` and `MatchQuery` (`blendTermQuery`) (server/src/main/java/org/opensearch/index/query/, index/search/) at the OpenSearch tip the Java shell builds on (`fae98a3`).

- With a `fuzziness`, each term a text field's query analyses to is a fuzzy query (the `fuzzy` query's own build: edits from `fuzziness`, `prefix_length` default 0, `max_expansions` default 50, `fuzzy_transpositions` default true), joined by the `operator`, with `minimum_should_match` applied as for a plain `match`.
- A keyword field's whole text is one fuzzy query; a field that is neither text nor keyword fails "Can only use fuzzy queries on keyword and text fields".
- `prefix_length`, `max_expansions` and `fuzzy_transpositions` are read without a `fuzziness` and change nothing, as in OpenSearch.
- Not here: `fuzzy_rewrite`, `lenient`, `auto_generate_synonyms_phrase_query` and `_name` stay refused, and `fuzziness` on `multi_match` and `match_phrase`.

## Acceptance criteria

- AC-1: a `match` with `fuzziness` on a text field builds one fuzzy query a term, joined by the operator.
- AC-2: the fuzzy options reach the fuzzy query, and a keyword field takes one fuzzy query for its text.
- AC-3: a number field fails as OpenSearch fails; a `match` without `fuzziness` builds as before.

## S3 request budget

None added: the fuzzy query runs over the segments the search already reads.

## Switch

None: a refusal becomes an answer.

## Tests

`crates/reader/src/dsl/queries/build_tests.rs` and `crates/node/tests/it/search_match_fuzziness.rs` through HTTP. No stock recording exists, so the expected parts are derived from the Java named above and are provisional. The earlier refusal of `fuzziness` in `query_tests.rs` is replaced: the change makes it an answer, and `fuzzy_rewrite` keeps the refusal's check.
