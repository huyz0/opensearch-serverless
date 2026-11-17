# Reader: match_phrase and multi_match

## Serves

Parity task R10.1 (search breadth), second part; invariant I-13 (REST matches OpenSearch's recordings); ADR 0004 (bounded memory).

## Change

`oss-reader` parses and runs `match_phrase` and `multi_match`, ported from their builders' `fromXContent` and from `MatchQuery.parse` (phrase type), `MultiMatchQuery.parse` and `QueryParserHelper.resolveMappingFields` at OpenSearch 3.10.0 (`crates/reader/src/dsl/queries/full_text.rs`, `full_text_build.rs`). A phrase on a text field is a Lucene phrase query with its slop; on any other field it is that field's term query. `multi_match` builds each resolved field's query (patterns expanded over the mapping, boosts multiplied for a field named twice) and combines them in a `dis_max` with the type's tie breaker.

Not in this change: `multi_match` types `cross_fields`, `phrase_prefix` and `bool_prefix`, its fuzzy parameters, an `analyzer` other than `standard`, `_name`, and a `multi_match` without `fields` (which reads `index.query.default_field`): all refused as not served yet.

## Acceptance criteria

- AC-1: every `_search` step of `fixtures/compat/search/match.json` (the 8 recorded before and 20 more for this change: phrases exact, out of order, sloppy, boosted, on a keyword, all-stopword with `zero_terms_query`, unmapped; `multi_match` best, most and phrase types, boosts, a pattern, `operator`, `lenient` and not, unmapped; and their parse errors) gets the status and body OpenSearch 3.10.0 gave, replayed against a reader node over the same blobs.
- AC-2: each parser reads the short and long forms and every parameter served, gives OpenSearch's message for a bad shape (negative slop, missing text, bad `zero_terms_query`, unknown type, bad field boost, unknown parameter), refuses what it does not serve with "not supported by this node yet", including a `fields` list of more than 4,096 entries (a bound of this node; OpenSearch bounds only the fields they resolve to, which a 1,000-field mapping keeps under its 1,024).
- AC-3: each query builds the Lucene query OpenSearch builds: a phrase with slop on text, a term elsewhere, nothing on an unmapped field; `multi_match`'s per-field queries boosted and combined with the type's tie breaker, one field left alone, patterns resolved, a field that cannot take the text matching nothing when lenient and failing the query otherwise.

## Test plan

AC-1 by the replay (`crates/compat/tests/node_search.rs`), seen red with a sloppy phrase's recorded score changed. AC-2 and AC-3 by `dsl/queries/full_text_tests.rs`.

## S3 request budget

Unchanged in kind: a phrase reads the positions of the segment's postings through the page cache by ranged GETs, once a page, as any query reads its postings. LIST 0, HEAD 0, PUT 0. Idle: 0.

## Switch

None: a query the reader refused before is now answered as OpenSearch answers it, with no downside to measure.

## Risks

lucene-rust's sloppy phrase matching and scoring must agree with Lucene's; the replay checks one sloppy phrase and two exact ones to the float.
