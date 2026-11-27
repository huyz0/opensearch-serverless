# `typed_keys` on `_msearch`

## Serves

The last `typed_keys` refusal of [search-typed-keys-metrics](../search-typed-keys-metrics/SPEC.md): `_msearch?typed_keys=true` answered 400 (unknown parameter), and the official clients send it on every multi search with aggregations.

## Change

- `typed_keys` is a URL parameter of `_msearch` and `_msearch/template`, with and without an index. `true` writes every item's aggregation keys as `type#name`, with the rules of the `_search` specs; `false` or none changes nothing; a non-boolean answers the boolean-parse 400 for the request.
- The parameter is applied to each item's source as the `_search` URL parameter is, so it holds over one index and over several.
- Not here: a `typed_keys` header key, which stays refused as an unknown header key.

## Acceptance criteria

- AC-1: a `_msearch?typed_keys=true` of two items with a `min` and a `terms` aggregation writes `min#lo` and a `#by` key typed for the terms aggregation in both items; without the parameter, or with `false`, no key holds a `#`; `typed_keys=maybe` answers 400.

## S3 request budget

None added: only the keys written change.

## Switch

None: it turns a refusal into an answer.

## Tests

`crates/node/tests/it/search_url_params.rs` through HTTP.
