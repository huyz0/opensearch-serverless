# The `stored_fields` key of a search body

## Serves

The `stored_fields` refusal of [compatibility](../../compatibility.md). Clients send `"stored_fields":"_none_"` to ask for ids-free, source-free hits (a count of matches with their scores), and `"stored_fields":[]` or a list of names to switch the hit's `_source` off; each answered 400.

## Change

Ported from `StoredFieldsContext.fromXContent`, `SearchService.parseSource` and `FetchPhase.createStoredFieldsVisitor` (server/src/main/java/org/opensearch/search/), at the OpenSearch tip the Java shell builds on (`fae98a3`).

- `"stored_fields":"_none_"` (or `["_none_"]`): the hit carries no `_id` and no `_source`: no stored field is fetched. With a `_source` key other than `false`, or with `fields`, OpenSearch fails the search; here that is refused as not supported yet.
- `"stored_fields":["a","b"]`, `"x"` or `[]`: only the named stored fields are fetched, and `_source` only when `"_source"` is among the names or the body has a `_source` key; without either, the hit has `_id` and no `_source`. A name that is no mapped field adds nothing. `"_source"` among the names switches `_source` on even over `"_source":false`, with the body's includes and excludes.
- A name that is an object (or `nested`) path fails as OpenSearch fails: "field [x] isn't a leaf field" (400), only when there is a hit to fetch.
- A name or pattern that matches a field mapped `store: true` is refused as not supported yet: stored field values are not read.
- `_none_` with other names is refused as not supported yet, and so is a `stored_fields` that is not a string or an array of strings.
- Not here: the `stored_fields` URL parameter, which stays unread.

## Acceptance criteria

- AC-1: with `_none_`, hits have no `_id` and no `_source`, and their other parts (`_index`, `_score`, `sort`) are unchanged.
- AC-2: with names, hits have `_id` and no `_source` unless `_source` is named or asked for by key; `[]` is the same.
- AC-3: an object name answers 400 "field [x] isn't a leaf field", a `store: true` match answers 400 "not supported by this node yet", and so do the refused combinations.

## S3 request budget

None added: the key only drops parts of the hit the fetch already holds.

## Switch

None: refusals become answers, and a search without the key writes as before.

## Tests

`crates/reader/src/dsl/source_tests.rs` and `crates/node/tests/it/search_stored_fields.rs` through HTTP. No recording from stock OpenSearch exists for these cases, so the expected parts are derived from the Java named above and are provisional until a fixture is recorded.
