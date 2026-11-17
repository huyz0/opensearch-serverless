# Reader: query_string

## Serves

Parity task R10.1 (queries), the `query_string` query; invariant I-13 (REST matches OpenSearch's recordings); ADR 0004 (bounded memory).

## Change

`oss-reader` serves `query_string` at OpenSearch 3.10.0, from `QueryStringQueryBuilder.fromXContent` and `doToQuery` and `QueryStringQueryParser` (`crates/reader/src/dsl/queries/query_string.rs` parses the body, `query_string_build.rs` builds the query). The grammar is the classic parser of the engine's `lucene-queryparser` (`classic::QueryParser`), which now takes the overrides of a subclass per parse (`parse_with_overrides`, an `Overrides` trait: one method each for the field, phrase, range, wildcard, regexp, prefix, fuzzy, bare-fuzzy and boolean queries Java's `QueryStringQueryParser` overrides). `query_string_build.rs` is that subclass.

- Keys: `query`, `fields` (names with `^boost` and patterns), `default_field`, `default_operator`, `type` (`best_fields`, `most_fields`, `phrase`), `tie_breaker`, `phrase_slop`, `allow_leading_wildcard` (true when absent, as the index setting's default), `lenient` (true when the fields are `*`), `minimum_should_match`, `fuzziness`, `fuzzy_prefix_length`, `fuzzy_max_expansions`, `fuzzy_transpositions`, `escape`, `boost`; `analyzer: standard`, `analyze_wildcard: false`, `enable_position_increments: true`, `max_determinized_states` and `auto_generate_synonyms_phrase_query` are read and have no effect.
- No `fields` and no `default_field` searches every field (`*`); `fields` together with `default_field` is a validation failure.
- A word or phrase is a `multi_match` of the query's `type` over the fields (a quoted phrase is `phrase` with its `~slop`, else `phrase_slop`); a text that analyzes to nothing in every text field is dropped, as `ZeroTermsQuery.NULL` drops it. A wildcard, prefix, regexp, fuzzy term or range is one query a field, lower-cased by a text field's analyzer where Java lower-cases it, combined by `dis_max` with the `type`'s tie breaker (or `tie_breaker`), each field's boost applied.
- `field:*` is a field-exists query, `*:*` matches everything; a field a name or pattern resolves to nothing matches nothing. A blank query matches nothing. A boolean of only prohibited clauses also matches every document (as a filter), at any depth.
- `~` alone takes its distance from `fuzziness`; `~n` takes `n` as a number, at most 2 (`0.5` is 0, `1.5` is 1); anything else is a `number_format_exception` on the upper-cased text.
- `field:>n`, `>=`, `<`, `<=` (and the same for the parser's default field) are ranges open at the other end; `[a TO b]`, `{a TO b}` and `*` as a bound are ranges of the field's type. An open end is unbounded whatever its inclusive flag.
- `lenient` turns a field's failure to build (a word in a number field, a wildcard on a number field) into a query that matches nothing; `minimum_should_match` applies to a top-level boolean only.
- Errors: a parse error is a `query_shard_exception` `Failed to parse query [q]` caused by the parser's `parse_exception` (with its own cause), a bad regexp fails with `unexpected end-of-string` and its kin, `[query_string] must be provided with a [query]`, `query does not support [k]`, `unknown token`, the operator and type messages.

Not in this change, refused as not supported yet: `analyze_wildcard: true`, an `analyzer` or `quote_analyzer` other than `standard`, `quote_field_suffix`, `rewrite`, `fuzzy_rewrite`, `time_zone`, `_name`, `enable_position_increments: false`, a negative boost, the types `cross_fields`, `phrase_prefix` and `bool_prefix`, and a range of strings in the syntax (`title:[a TO b]`, `title:>x`), which the `range` query refuses too. The disjuncts of a `dis_max` over several fields are in the mapping's order, not Java's `HashMap` order; scores are the same.

The engine dependency moves to the `huyz0/lucene-rust` branch `claude/project-thread-qcdb96`, at the revision that adds `Overrides` (a fork branch of the pinned revision, no other change); all `lucene-*` dependencies move together.

## Acceptance criteria

- AC-1: every `_search` step of `fixtures/compat/search/match.json` gets the status and body OpenSearch 3.10.0 gave, replayed against a reader node over the same blobs. The 103 new steps (named `qs ...`) cover words, operators, groups, negation, fields, boosts, types, phrases and slop, wildcards, prefixes, regexps, fuzzy terms and their settings, numeric and date ranges, comparisons, `field:*`, `*`, empty and blank queries, unmapped fields, `minimum_should_match`, numeric fields with and without `lenient`, `escape`, `boost`, a `bool` filter and the errors.
- AC-2: the parser reads every key and its default and fails as OpenSearch fails.
- AC-3: the leaves build the query OpenSearch builds for each form (compared with `_validate/query?explain=true`).
- AC-4: what is not served is refused with one message.

## Test plan

- AC-1: the replay (`crates/compat/tests/node_search.rs`).
- AC-2 and AC-4: `dsl/queries/query_string_tests.rs`.
- AC-3: `dsl/queries/query_string_build_tests.rs`.

## S3 request budget

The query changes only what is matched in the segments the search already reads. LIST 0, HEAD 0, PUT 0, no new GET. Idle: 0.

## Memory

The text is held once (at most the body's size, which the HTTP edge bounds); the grammar's clause limit (1,024 a boolean) and nesting limit bound the query, and one query is built a field a leaf, at most the mapping's fields.
