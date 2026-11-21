# A node searches a point in time of several shards

## Serves

R10.6: [node-frozen-search](../node-frozen-search/SPEC.md) refused a view of more than one shard. An index of several shards is the normal case, so a point in time over one now answers as a live search over its shards does ([node-coordinate](../node-coordinate/SPEC.md)).

## Change

- `SearchService::query_frozen_reserving` runs a query on shard `n` of a view, opened as a single-shard search of a view opens it (kept in the open-indexes pool under `pit:<id>#<n>`); a frozen shard has no overlay. `query_shard_reserving` and it share one body.
- `SearchService::coordinate_view` runs the coordinator's merge over the view's shards (the merge's shard `n` is the view's `n`th), and `handle_view` writes the merged response with `pit_id` first, as a search of one shard of a view does.
- A view of one shard takes the existing path unchanged. A node built without the `coordinator` feature still refuses a view of several shards (400).

## Acceptance criteria

- AC-1: a search of a two-shard view answers what a live search of the same two shards answers, with `pit_id` first.
- AC-2: it does not read the live commits: with both shards' commit registers gone, a live search fails and the search of the view answers, both shards successful.
- AC-3: a second search of the view makes no request to the index's store and holds one open-indexes entry a shard.

## Test plan

`crates/node/tests/it/frozen_search.rs`, over the compat fixtures with the index made two shards; AC-1 observed failing with the id the response names altered.

## S3 request budget

Cold: the view's read, then per shard the descriptor's one GET and the engine's ranged reads, as a live shard opens. Warm: the view's read only. No LIST, no PUT, and the commit registers are never read.

## Risks

- The shards of a view open on the node that gets the request; with many shards that is an open each, charged to the `open_indexes` pool, which closes the least recently searched to make room.
- A shard the pool cannot hold fails its own query with the pool's circuit-breaking error, as for a live search over many shards; what the merge does with a failed shard is the live search's.
