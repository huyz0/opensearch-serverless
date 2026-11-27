# `routing` and `preference` on `_count`

## Serves

The `_count` half of [search-routing](../search-routing/SPEC.md): `?routing=` and `?preference=` on `/{index}/_count` answer 400 (unknown parameter) while `_search` takes them.

## Change

- `routing` narrows a count of an index of several shards to the shards its values route to, as `_search` does (`shard_plan`); a routing partition, an empty value or a one-shard index counts every shard.
- `preference` is accepted and has no effect: a shard has no copies.
- No new request: the descriptor is the one the count already opened.
- Not here: `routing` and `preference` on a point-in-time search or a point-in-time create (see [node-pit-route](../node-pit-route/SPEC.md)).

## Acceptance criteria

- AC-1: on a two-shard index, `/{index}/_count?routing=<v>` counts only the shard `<v>` routes to, and both values counted together count both shards.
- AC-2: an empty `routing`, or no `routing`, counts every shard; on a one-shard index any routing counts what the count without it counts.
- AC-3: `?preference=<any string>` answers what the count without it answers.

## S3 request budget

None added.

## Switch

None: it turns a refusal into an answer and visits no more shards than before.

## Tests

`crates/node/tests/it/search_routing.rs`, through HTTP on the two-shard index of [search-routing](../search-routing/SPEC.md), whose shards hold copies of the same two matching documents (a count of 4 means both shards, 2 means one).
