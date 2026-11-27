# `ignore_unavailable` on `_field_caps` and `_msearch`

## Serves

The `ignore_unavailable` gap of [compatibility](../../compatibility.md): `_search` and `_count` skip a name that is no index ([search-several-indexes](../search-several-indexes/SPEC.md)); `_field_caps` answers 400 (unknown parameter) and an `_msearch` header naming it answers its item "not supported by this node yet".

## Change

- `?ignore_unavailable=true` on `_field_caps` drops the names that are no index before the capabilities are read; with none left it answers `{"indices":[],"fields":{}}`. Absent or `false`, a missing name answers 404 as before.
- `"ignore_unavailable"` in an `_msearch` header (a JSON boolean or a string) does the same for its item: a missing name is skipped, and with none left the item answers an empty result with status 200.
- Any other value answers the boolean-parse 400 (`Failed to parse value [..] as only [true] or [false] are allowed.`), as the item's error for `_msearch`.
- The check is the one `_search` makes: it opens each name once, a read the search or the capabilities needed anyway.

## Acceptance criteria

- AC-1: `_field_caps` over `compat-term,no-such-index` with the parameter answers what `compat-term` alone answers, and without it answers 404.
- AC-2: `_field_caps` over only missing names answers `{"indices":[],"fields":{}}`; a value that is not a boolean answers 400.
- AC-3: an `_msearch` item whose header has `"ignore_unavailable":true` (or `"true"`) over a list with a missing name answers the total of the others; without it the item answers 404.
- AC-4: an item whose names are all missing answers total 0 with status 200; a non-boolean value answers an item with status 400.

## S3 request budget

None added: the existence check reads the descriptor the search or the capabilities read next, and the open is cached.

## Switch

None: it turns a refusal into an answer.

## Tests

`crates/node/tests/it/search_several.rs`, through HTTP.
