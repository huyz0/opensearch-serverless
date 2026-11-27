# The `match_phrase_prefix` query

## Serves

The `match_phrase_prefix` refusal of [compatibility](../../compatibility.md): a stock-OpenSearch comparison found it answered 400 here.

## Change

Ported from `MatchPhrasePrefixQueryBuilder.fromXContent` and `doToQuery`, `MatchQuery.parse` of type `PHRASE_PREFIX`, and `TextFieldMapper.createPhrasePrefixQuery` (server/src/main/java/org/opensearch/index/query/, index/search/, index/mapper/) at the OpenSearch tip the Java shell builds on (`fae98a3`).

- On a text field the text is analysed into positions; each is a whole term but the last, which is a prefix expanded over the terms the reader holds (`MultiPhrasePrefixQuery.rewrite`), at most `max_expansions` of them (50 by default), in `slop` (0 by default). One term is a phrase of one position with a prefix.
- The expansion is the lucene-rust `MultiPhrasePrefixQuery::resolve`, which walks every segment's terms of the prefix.
- An empty analysis matches nothing, or everything with `zero_terms_query: all`.
- On any other field type the query fails as OpenSearch does: `Can only use phrase prefix queries on text fields - not on [f] which is of type [t]`.
- A text that analyses to synonyms at one position is refused as not supported yet; `_name` and an `analyzer` other than `standard` likewise.
- `requires fieldName`, `requires query value`, the multiple-fields error, a negative `slop` or `max_expansions`, and an unknown key fail as the Java fails.

## Acceptance criteria

- AC-1: a text field's terms become one multi-phrase prefix query with `slop` and `max_expansions`, whole terms before the last.
- AC-2: another field type fails as OpenSearch fails, an empty analysis follows `zero_terms_query`, and what is not served is refused.
- AC-3: through HTTP, the last term matches by prefix within the phrase.
- AC-4: the query scores with its field's norms: the field is one the search opens norms for, and the scores equal stock 3.10.0's (`fixtures/compat/search-breadth-gaps/queries.json`, step `match_phrase_prefix`).

## S3 request budget

None added: the query runs over the segments the search already reads.

## Switch

None: a refusal becomes an answer.

## Tests

`crates/reader/src/dsl/queries/phrase_prefix_tests.rs` and `crates/node/tests/it/search_match_phrase_prefix.rs` through HTTP. No stock recording exists, so the expected parts are derived from the Java named above and are provisional.
