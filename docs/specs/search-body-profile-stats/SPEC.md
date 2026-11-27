# `profile: false` and `stats` in a search body

## Serves

The body-keys part of the `_search` refusals of [compatibility](../../compatibility.md). Clients send `"profile":false` as a default and `"stats":["group"]` to label a search for the stats API, and each answered 400.

## Change

- `profile` of `false` is read and changes nothing, as `SearchSourceBuilder` reads it. `profile: true` stays refused: a profile answer is not built.
- `stats` as an array of strings is read and changes nothing: this node keeps no per-group search statistics, and the search answers as it would without the key. An element that is not a string is refused as not supported.
- Not here: `stored_fields`, `fields`, `indices_boost` and `ext`, which stay refused.

## Acceptance criteria

- AC-1: a body with `"profile":false` and one with `"stats":["a","b"]` (alone or together) answer 200 with the body the same search answers without them (`took` aside).
- AC-2: `"profile":true` and a `stats` element that is not a string answer 400 "not supported by this node yet".

## S3 request budget

None added: the keys are read and dropped.

## Switch

None: refusals become answers, and a body without the keys reads as before.

## Tests

`source_tests.rs` in the reader and `crates/node/tests/it/search_body_keys.rs` through HTTP.
