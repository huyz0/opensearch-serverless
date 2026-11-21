# A node searches a point in time

## Serves

R10.6, node wiring: `POST|GET /_search` with `{"pit":{"id":...}}` runs the search over the commit the view froze, whatever the live commit has become. Follows [reader-frozen-view](../reader-frozen-view/SPEC.md), [reader-search-pit](../reader-search-pit/SPEC.md) and [node-pit-route](../node-pit-route/SPEC.md); the cache of open views is the open-indexes pool's ([ADR 0014](../../adr/0014-open-indexes-pool.md)).

## Change

`oss-node`'s `SearchService` gains `set_point_in_time(store, clock, buffer_bytes)` and a `/_search` route (no index in the path). A search that names a point in time:

- reads the view (one request), which must be live and unexpired, else 404 `search_context_missing_exception`;
- extends it to `now + keep_alive` when the body gives one (at most one hour, as a create), one more request;
- opens the view's frozen shard as a search of an index opens one, charged to the `open_indexes` pool, and keeps it open under the view's id and shard, so a later search of the same view opens nothing and the least recently searched is closed to make room;
- answers as a live search of the shard does, with the response's `pit_id` the view's id and the index named in each hit the view's index.

Any index in the path with a `pit` is refused (400), a `/_search` with no `pit` is refused as not supported (400), a node that has no point-in-time store refuses a `pit` as not supported (400), and a view of more than one shard is searched as [node-frozen-search-shards](../node-frozen-search-shards/SPEC.md) says (a node built without the coordinator role refuses it, 400). The view's buffer is reserved from the request-body pool before it is allocated.

Not here, listed: forwarding to the node that holds a view open (any node opens a view from its record: nothing is owned).

## Acceptance criteria

- AC-1: a search of a view answers what a live search of the same shard answers, with `pit_id` first, for the view of the commit the live search is open on.
- AC-2: it does not read the live commit: with the shard's commit register gone, a live search fails and the search of the view answers.
- AC-3: a second search of the view makes no request to the index's store: the open shard is kept, held by one entry of the open-indexes pool.
- AC-4: an id that names no view, a released one and an expired one answer 404 `search_context_missing_exception`.
- AC-5: `keep_alive` moves the view's deadline to now plus it, and one over an hour is 400.
- AC-6: an index in the path, a missing `pit`, and a node with no point-in-time store each answer 400, and read no index.
- AC-7: the view's buffer is charged to the request-body pool: a pool without room for it answers 429 before any request.

## Test plan

`crates/node/tests/it/frozen_search.rs` over the compat fixtures in the emulator.

## S3 request budget

Cold: one read of the view, the descriptor's one GET, the engine's ranged reads; with a keep-alive, one more request to extend. Warm: the view's read only (the shard is open). LIST 0, HEAD 0, PUT 0 but the extension. The commit register is never read.

## Risks

- A view is read on every search, so a view's lifetime is the store's, not this node's cache: a released view stops answering at its next search, while its shard stays open until closed for room.
