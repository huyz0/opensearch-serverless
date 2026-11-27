# `rest_total_hits_as_int` inside `top_hits` and `inner_hits`

## Serves

The one difference [search-total-hits-as-int](../search-total-hits-as-int/SPEC.md) left: the total inside a `top_hits` aggregation and inside `inner_hits` stayed `{"value":N,"relation":"eq"}`. OpenSearch's `SearchHits.toXContent` reads the parameter for every `hits` object of the response, so a legacy client sees a number in each.

## Change

- With `rest_total_hits_as_int=true` the `total` of every `top_hits` aggregation (top level or under a bucket) and of every `inner_hits` entry is a number, on a search of one index, of several, and in an `_msearch` item. `false` or none changes nothing.
- A scope over one write carries the setting (`oss_reader::hits_total`, as `typed_keys` does), set where a response is written: the shard's writer, the coordinator's merge, and the shard-side render of the hits a coordinator asks for.
- Not here: the total of a response a remote shard writes for a forwarded search.

## Acceptance criteria

- AC-1: over one index a `top_hits` at the top level and one under a `terms` bucket write `"total":N`, and the response's own total is unchanged; `rest_total_hits_as_int=false` writes the objects.
- AC-2: the same over several indexes.
- AC-3: the `inner_hits` of a nested query write `"total":N` under the setting and the object without it.

## S3 request budget

None added: only how a total is written changes.

## Switch

None: it corrects a total that stayed an object where OpenSearch writes a number.

## Tests

`crates/node/tests/it/search_url_params.rs` through HTTP, and the `hits_total` and inner-hits unit tests of the reader.
