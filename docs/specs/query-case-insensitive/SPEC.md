# `case_insensitive` on `term`, `prefix` and `wildcard`

## Serves

The `[case_insensitive] true` refusal of [compatibility](../../compatibility.md): a stock-OpenSearch comparison found these queries answered 400 here.

## Change

Ported from `AutomatonQueries.caseInsensitiveTermQuery`, `caseInsensitivePrefix` and `toCaseInsensitiveWildcardQuery` with the `StringFieldType` queries that call them (server/src/main/java/org/opensearch/), at the OpenSearch tip the Java shell builds on (`fae98a3`).

- `term`, `prefix` and `wildcard` with `"case_insensitive": true` on a text or keyword field match terms in either case: an automaton query over the field's terms, constant-scored under the default rewrite. In a wildcard pattern `*` is any run, `?` one code point, and `\` escapes the next code point (a trailing `\` is itself).
- `prefix` and `wildcard` on another field type fail as they do without the flag; a `term` on another type is refused as not supported yet.
- `case_insensitive: false` or absent builds as before.
- Not here: `regexp` with `case_insensitive: true`, which stays refused.

## Acceptance criteria

- AC-1: a pattern matches in either case, `*`, `?` and escapes keeping their meaning.
- AC-2: through HTTP, `term`, `prefix` and `wildcard` with the flag match a keyword term of another case, and without it do not.
- AC-3: `regexp` with the flag and a `term` on a number field are refused.

## S3 request budget

None added: the query runs over the segments the search already reads.

## Switch

None: refusals become answers.

## Tests

`crates/reader/src/dsl/queries/case_insensitive_tests.rs`, `parse_tests.rs` and `crates/node/tests/it/search_case_insensitive.rs`. No stock recording exists, so the expected parts are derived from the Java named above and are provisional. The refusals of `prefix` and `term` with the flag, pinned in `parse_tests.rs` and `query_tests.rs`, are replaced: the change makes them answers, and `regexp` keeps the refusal's check.
