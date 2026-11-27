# `preference` on a point-in-time create, `routing` and `preference` on a point-in-time search

## Serves

The point-in-time half of [search-routing](../search-routing/SPEC.md). A create with `?preference=` answered 501, and a search of a view with `routing` or `preference` ignored them silently.

## Change

- `POST /{index}/_search/point_in_time?preference=<any>` and `/_pit` are taken and the preference has no effect: a shard has no copies to prefer. An empty one stays no refusal.
- A search naming a point in time with `routing` or `preference` is 400 `illegal_argument_exception` naming the parameter ("[routing] cannot be used with point in time"), as OpenSearch refuses it: the view already holds its shards. The refusal comes before any read.
- Not here: `routing` on a create stays 501, since it would freeze a subset of the index's shards and the view records every shard of one index.

## Acceptance criteria

- AC-1: a create with `preference=_local` answers 200 and freezes every shard.
- AC-2: a search of a view with `routing` or with `preference` answers 400 naming the parameter, and makes no object-store or view-store request.
- AC-3: a create with a non-empty `routing` is still 501 `unsupported_point_in_time`.

## S3 request budget

None added; AC-2 is checked against the store meters.

## Switch

None: a refusal becomes an answer, and a silently ignored parameter becomes the refusal OpenSearch gives.

## Tests

`crates/node/tests/pit_route_limits.rs` and `crates/node/tests/it/frozen_search.rs`; AC-3 is held by `what_cannot_be_frozen_is_refused_with_the_javas_status` in `crates/node/tests/pit_route.rs`.
