# An empty `ext` in a search body

## Serves

The body-keys part of the `_search` refusals of [compatibility](../../compatibility.md). Clients send `"ext":{}` as a default, and it answered 400.

## Change

- `"ext":{}` is read and changes nothing, as `SearchSourceBuilder.fromXContent` reads an `ext` object with no section: no `SearchExtBuilder` is built.
- A section inside `ext` names a search extension a plugin registers; this node has none, so it stays refused as not supported yet.
- Not here: `indices_boost`, which stays refused: its names resolve across the cluster's indexes and aliases, and the boost reaches every shard of a coordinated search.

## Acceptance criteria

- AC-1: `"ext":{}` parses and the search answers as without the key.
- AC-2: an `ext` with a section is refused as not supported by this node yet.

## S3 request budget

None added.

## Switch

None: a refusal becomes an answer, and a search without the key writes as before.

## Tests

`crates/reader/src/dsl/source_tests.rs` and `crates/node/tests/it/search_body_keys.rs` through HTTP. Derived from the Java at `fae98a3`; no stock recording exists, so provisional.
