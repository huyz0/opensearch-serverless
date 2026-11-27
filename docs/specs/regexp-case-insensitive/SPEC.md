# `case_insensitive` on `regexp`

## Serves

The `[regexp] [case_insensitive] true` refusal of [compatibility](../../compatibility.md); [query-case-insensitive](../query-case-insensitive/SPEC.md) left it as the one query of the four still refused.

## Change

Ported from `RegexpQueryBuilder.doToQuery` and `StringFieldType.regexpQuery` (server/src/main/java/org/opensearch/index/), which pass `RegExp.ASCII_CASE_INSENSITIVE` as the match flag when `case_insensitive` is true (server/src/main/java/org/opensearch/index/query/RegexpQueryBuilder.java at the OpenSearch tip the Java shell builds on, `fae98a3`).

- `regexp` with `"case_insensitive": true` on a text or keyword field matches terms in either ASCII case: the pattern is compiled with Lucene's `ASCII_CASE_INSENSITIVE` match flag and run as an automaton query over the field's terms, constant-scored under the default rewrite.
- Another field type fails as it does without the flag; the `index.max_regex_length` bound applies as before.
- A pattern Lucene's `RegExp` cannot parse, or one too complex to determinize, answers 400 `illegal_argument_exception` with the engine's message.
- `case_insensitive: false` or absent builds as before.

## Acceptance criteria

- AC-1: a pattern matches a keyword term of another ASCII case, and the same pattern without the flag does not.
- AC-2: through HTTP, the flag matches a mixed-case term and honours `.`, `*`, `[a-c]` and alternation.
- AC-3: a pattern that does not parse and a regexp on a number field answer 400.

## S3 request budget

None added: the query runs over the segments the search already reads.

## Switch

None: a refusal becomes an answer.

## Tests

`crates/reader/src/dsl/queries/case_insensitive_tests.rs`, `parse_tests.rs` and `crates/node/tests/it/search_case_insensitive.rs`. No stock recording exists, so the expected parts are derived from the Java named above and are provisional; the error text of an unparseable pattern follows the engine, not a recording.
