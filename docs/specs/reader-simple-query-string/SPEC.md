# Reader: simple_query_string

## Serves

Parity task R10.1 (queries), the `simple_query_string` query; invariant I-13 (REST matches OpenSearch's recordings); ADR 0004 (bounded memory).

## Change

`oss-reader` serves `simple_query_string` at OpenSearch 3.10.0, from `SimpleQueryStringBuilder.fromXContent` and `doToQuery`, `SimpleQueryStringQueryParser` and Lucene's `SimpleQueryParser` (`crates/reader/src/dsl/queries/simple_string.rs` parses the body, `simple_string_build.rs` is the grammar and `simple_string_leaf.rs` builds each leaf). The grammar is a statement-for-statement port of Lucene's, kept here because the engine's port has no hook for OpenSearch's leaf queries.

- Keys: `query`, `fields` (names with `^boost` and patterns, default `*`), `default_operator`, `flags` (names joined by `|`, or a number), `lenient`, `minimum_should_match`, `fuzzy_prefix_length`, `fuzzy_max_expansions`, `fuzzy_transpositions`, `boost`; `analyzer: standard`, `analyze_wildcard: false`, `auto_generate_synonyms_phrase_query`, `all_fields`, `locale` and `lowercase_expanded_terms` are read and have no effect, as in OpenSearch.
- A word is a `most_fields` match over the resolved fields; a phrase is a `phrase` match with its `~slop`; a prefix and a fuzzy term are built once a field, lower-cased by the field's analyzer, and combined by `dis_max` with a tie breaker of 1. A field with a boost wraps its query in it.
- `+`, `|`, `-`, parentheses, `\` escapes, whitespace splitting, `*` prefixes, `~n` fuzziness and `"..."~n` phrases follow the `flags`; a lone `*` is `match_all`, an empty query matches nothing.
- `lenient` (true when the fields are `*`) turns a field's failure to build into a query that matches nothing.
- `minimum_should_match` applies to a top-level boolean only.
- More than 1,024 clauses fail with `too_many_clauses`.
- Errors: `[simple_query_string] query text missing`, `unsupported field [k]`, `query does not support [k]` (an array), `Unknown simple_query_string flag [x]`, the operator and boolean parse messages, each at the column Java reports.

Not in this change, refused as not supported yet: `analyze_wildcard: true`, an `analyzer` other than `standard`, `quote_field_suffix`, `_name`, a negative boost (the query's or a field's), an empty field name, and a group nested more than 128 deep (OpenSearch allows 1,000; each level here recurses on the native stack). The disjuncts of a `dis_max` over several fields are in the mapping's order, not Java's `HashMap` order; scores are the same.

## Acceptance criteria

- AC-1: every `_search` step of `fixtures/compat/search/match.json` gets the status and body OpenSearch 3.10.0 gave, replayed against a reader node over the same blobs. The 55 new steps (named `sqs ...`) cover words, operators, negation, groups and their errors, phrases and slop, prefix, fuzzy, escapes, several fields and boosts, empty and blank queries, `*`, unmapped fields, `minimum_should_match`, numeric fields with and without `lenient`, every flag, the fuzzy settings, `boost`, a `bool` filter and the parse errors.
- AC-2: the parser reads every key and its default, flags as names or numbers, and fails as OpenSearch fails.
- AC-3: the grammar builds the query OpenSearch builds for each form (compared with `_validate/query?explain=true`), including `+`, `|`, `-`, groups, phrases, prefix, fuzziness and each flag alone.
- AC-4: the leaves over several fields, boosts, numeric and unmapped fields, `lenient`, `minimum_should_match` and the clause limit.
- AC-5: what is not served is refused with one message.

## Test plan

- AC-1: the replay (`crates/compat/tests/node_search.rs`).
- AC-2 and AC-5: `dsl/queries/simple_string_tests.rs`.
- AC-3 and AC-4: `dsl/queries/simple_string_build_tests.rs`.

## S3 request budget

The query changes only what is matched in the segments the search already reads. LIST 0, HEAD 0, PUT 0, no new GET. Idle: 0.

## Memory

The text is held once as UTF-16 (at most the body's size, which the HTTP edge bounds) and each clause is a small allocation; a query has at most 1,024 clauses a boolean, and a group nests at most 128 deep.
