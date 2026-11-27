# `typed_keys`: `nested`, `reverse_nested` and `missing`

## Serves

The last part of [search-typed-keys-metrics](../search-typed-keys-metrics/SPEC.md): `nested` and `reverse_nested` answered 400 beside `typed_keys`, because their result is held as a `filter`'s is. The `missing` aggregation, served since [search-typed-keys-buckets](../search-typed-keys-buckets/SPEC.md), is held the same way and was written `filter#name` where OpenSearch writes `missing#name`.

## Change

- A `FiltersResult` records which aggregation it is of (`filters`, `filter`, `missing`, `nested` or `reverse_nested`); the reduce keeps it and refuses to add results of different kinds.
- With `typed_keys=true` the key is `nested#name`, `reverse_nested#name`, `missing#name`, `filter#name` or `filters#name`, at the top level and as a sub-aggregation; the refusal of `nested` and `reverse_nested` is gone.
- Without `typed_keys` nothing is written differently.

## Acceptance criteria

- AC-1: one request holding `missing`, `filter`, `nested` and a `reverse_nested` and `max` under the `nested` answers `missing#ms`, `filter#f`, `nested#n`, `reverse_nested#back` and `max#m`, and without the parameter no key holds a `#`.
- AC-2: each kind writes its own type name, and a reduce of two results of different kinds is refused.

## S3 request budget

None added: only the keys written change.

## Switch

None: it turns a refusal into an answer and corrects a key that was wrong.

## Tests

`crates/node/tests/it/search_url_params.rs` through HTTP, and the `typed` and `filters` unit tests of the reader. The nested aggregations run over a path no index maps, which the reader counts as no documents; an index with nested objects is not in the node's HTTP fixtures.
