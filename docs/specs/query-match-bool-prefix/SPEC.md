# The `match_bool_prefix` query

## Serves

The `match_bool_prefix` refusal of [compatibility](../../compatibility.md): a stock-OpenSearch comparison found it answered 400 here.

## Change

Ported from `MatchBoolPrefixQueryBuilder.fromXContent` and `doToQuery`, and `MatchQuery.parse` of type `BOOLEAN_PREFIX` (server/src/main/java/org/opensearch/index/query/, index/search/) at the OpenSearch tip the Java shell builds on (`fae98a3`).

- On a text field the text is analysed; each term but the last is a term query and the last a prefix query, joined by `operator` (`OR` by default), with `minimum_should_match` applied. A single term is the prefix query alone.
- On a keyword field the whole text is a prefix query; on another type a term query, as in OpenSearch.
- An empty analysis matches nothing.
- OpenSearch makes the last term a prefix only when the text ends where the last token does. This node cannot read that from the analyser's offsets, so a text that ends in anything but a letter or digit is refused as not supported yet.
- `prefix_length`, `max_expansions` and `fuzzy_transpositions` are read and change nothing without a `fuzziness`. `fuzziness`, `fuzzy_rewrite`, `_name` and an `analyzer` other than `standard` are refused as not supported yet.
- `requires fieldName`, `requires query value`, the multiple-fields error and an unknown key fail as the Java fails.

## Acceptance criteria

- AC-1: the last term is a prefix query and the others term queries, joined by the operator.
- AC-2: a keyword field takes a prefix query of its text and another type a term query.
- AC-3: bad bodies fail as OpenSearch fails and what is not served is refused.

## S3 request budget

None added: the query runs over the segments the search already reads.

## Switch

None: a refusal becomes an answer.

## Tests

`crates/reader/src/dsl/queries/bool_prefix_tests.rs` and `crates/node/tests/it/search_match_bool_prefix.rs` through HTTP. No stock recording exists, so the expected parts are derived from the Java named above and are provisional.
