# Reader: term-level and compound queries

## Serves

Parity task R10.1 (search breadth), first part; invariant I-13 (REST matches OpenSearch's recordings); ADR 0004 (bounded memory).

## Change

`oss-reader` parses and runs `terms`, `prefix`, `wildcard`, `regexp`, `fuzzy`, `exists`, `ids`, `constant_score`, `dis_max` and `match_none`, ported from their builders' `fromXContent` and `doToQuery` and the field types' query factories at OpenSearch 3.10.0 (`crates/reader/src/dsl/queries/`). They nest inside `bool` and each other like the first subset's queries.

Not in this change: `terms` lookups and `value_type: bitmap`, `case_insensitive: true`, a `rewrite` other than the default, `regexp` flags other than `ALL`, `_name` (all refused as not served yet); the full-text queries (`match_phrase`, `multi_match`, `query_string`) and the hit features come in later changes. A keyword's index-or-doc-values query is searched by its index alone, which matches the same documents.

## Acceptance criteria

- AC-1: every `_search` step of `fixtures/compat/search/term.json` (the 9 recorded before and 32 more for this change: each new query on keyword, text, integer and date fields, boosts, unmapped fields, and their parse and shard errors) gets the status and body OpenSearch 3.10.0 gave, replayed against a reader node over the same blobs.
- AC-2: each parser gives OpenSearch's message for a bad shape (two fields, a missing field or value, an unknown parameter, a bad fuzziness, an empty `dis_max`), and refuses every parameter it does not serve with "not supported by this node yet".
- AC-3: each query builds the Lucene query OpenSearch builds for each field type: a term set for strings and point sets for numbers (values with a fraction dropped), constant-scored multi-term queries, fuzzy edits from `AUTO` by length, exists by doc values, norms or `_field_names`, by pattern and by object; a string query on another type is a `query_shard_exception` naming the index, a fuzzy one an `illegal_argument_exception`.
- AC-5: each query keeps OpenSearch's bounds at their defaults: at most 65,536 `terms` values, at most 1,024 clauses for a `terms` on a date, a `regexp` pattern of at most 1,000 characters, `fuzzy` expanding to at most 1,024 terms, and `max_determinized_states` only at its default; each over-bound gives OpenSearch's message.
- AC-4: an `ids` value is encoded as `Uid.encodeId` encodes it, and every encoding reads back through `response::write_id` as the id written.

## Test plan

AC-1 by the replay (`crates/compat/tests/node_search.rs`), seen red with a recorded score and a recorded column changed. AC-2 by `dsl/queries/parse_tests.rs`, AC-3 by `dsl/queries/build_tests.rs`, AC-4 by `dsl/queries/uid.rs`'s tests, AC-5 by `dsl/queries/build_tests.rs` and `parse_tests.rs`.

## S3 request budget

Unchanged in kind: the queries read the same segment files through the page cache by ranged GETs, once a page. `exists` on a keyword or number reads its doc values, which a query of the first subset did not. LIST 0, HEAD 0, PUT 0. Idle: 0.

## Switch

None: a query the reader refused before is now answered as OpenSearch answers it, with no downside to measure.

## Risks

lucene-rust's multi-term and fuzzy matching must agree with Lucene's (a regexp syntax it lacks fails the query); the replay covers one shape of each. Scores of `fuzzy` and `dis_max` depend on its BM25 and blended-frequency ports, which the replay checks to the float.
