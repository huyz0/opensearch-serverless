# `multi_match` of type `cross_fields`

## Serves

The `[multi_match] type [cross_fields]` refusal of [compatibility](../../compatibility.md): a stock-OpenSearch comparison found it answered 400 here.

## Change

Ported from `MultiMatchQuery.buildCrossFieldQuery`, its `BlendedQueryBuilder` and `blendTerms` (server/src/main/java/org/opensearch/index/search/) at the OpenSearch tip the Java shell builds on (`fae98a3`).

- The listed fields are grouped by analyser: text fields by their pinned analyser (the standard one otherwise), every other type as one keyword group.
- A group of one field builds as a `match` on it, with its field boost. A group of several analyses the text once, and each term becomes one blended term query over the group's fields (`BlendedTermQuery.dismaxBlendedQuery`, field boosts kept), joined by `operator` with `minimum_should_match` applied. A field whose term query is not a plain term query (a keyword's constant-scored one, a number's point query) sits beside the blend in a disjunction max.
- The groups combine as a disjunction max with the tie breaker, `0` by default.
- `lenient` (all-fields queries default to it) makes a field that cannot take the text match nothing.
- Not here: a text whose analysis makes synonyms is refused as not supported yet, and so are `cutoff_frequency`, `fuzziness` and the other options already refused for `multi_match`.

## Acceptance criteria

- AC-1: fields of one analyser blend each analysed term, keeping field boosts and the operator and tie breaker.
- AC-2: a group of one field is a plain `match`, and groups combine by disjunction max.

## S3 request budget

None added: the query runs over the segments the search already reads.

## Switch

None: a refusal becomes an answer.

## Tests

`crates/reader/src/dsl/queries/cross_fields_tests.rs`. There is no HTTP test: no recorded fixture holds two text fields, so the engine's blended scoring is exercised by its own tests and the built query is checked here. No stock recording exists, so the expected parts are derived from the Java named above and are provisional. The refusal of `cross_fields` pinned in `full_text_tests.rs` now checks `phrase_prefix`, which stays refused.
