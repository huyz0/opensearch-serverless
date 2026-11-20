# A writer node serves `_refresh` and the document get routes

## Serves

Parity plan R7.6, node assembly: `GET|HEAD /{index}/_doc/{id}`, `GET|HEAD /{index}/_source/{id}` and `POST|GET /{index}/_refresh`, answered as stock OpenSearch answers them for the shards this node writes. The writes are [node-doc-write-route](../node-doc-write-route/SPEC.md) and [node-bulk-route](../node-bulk-route/SPEC.md); a get is the realtime read that `?refresh` and the update action's merge need.

## Change

- `oss-writer`'s `index::Shard` gains `get(id, realtime)`: the live document of an id, as `InternalEngine.get` reads it. A realtime get of an id written since the last refresh refreshes first, so it sees the write; of an id the version map holds as deleted it answers none; a non-realtime get reads the last refresh. The answer carries the document's `_version`, `_seq_no`, `_primary_term` and stored `_source`.
- `oss-node`, with the `writer` feature, gains `get_route` (`GetService`, `routes`) and `refresh_route` (`RefreshService`, `routes`), and `WriterRole::get`.
- Parameters read: get takes `realtime`, `refresh`, `routing` and `_source` (`true`, `false` or empty); refresh takes none. Any other parameter is the router's `unrecognized parameter` 400.
- Not changed: a shard another node owns (a get answers 503 `no_shard_available_action_exception`, a refresh counts it failed): forwarding reads needs the transport action; `_source_includes`, `_source_excludes`, `stored_fields`, `version` and `preference`; `_refresh` with no index or a wildcard, which needs the index list; aliases.

## Acceptance criteria

- AC-1: a realtime get of an id written and not refreshed finds it, with its `_version`, `_seq_no`, `_primary_term` and `_source`; `realtime=false` before a refresh does not.
- AC-2: a get of a deleted id, or one never written, answers 404 `{"_index","_id","found":false}`; `HEAD` answers the same statuses with no body.
- AC-3: `_source=false` leaves `_source` out of `GET /{index}/_doc/{id}`; any other `_source` value is 400.
- AC-4: `GET /{index}/_source/{id}` answers the stored source alone, 404 for none.
- AC-5: a get of an unknown index is 404 `index_not_found_exception`; with `routing` it picks the shard stock's routing picks, and an index whose mapping requires routing refuses a get without one as 400 `routing_missing_exception`.
- AC-6: `_refresh` of an index makes every shard's writes visible to a non-realtime get and answers `{"_shards":{"total":N,"successful":N,"failed":0}}`; of an unknown index it is 404.
- AC-7: a shard another node owns is asked of its owner, as [read routing](../node-read-routing/SPEC.md) says; with a route that cannot name an owner's answer it answers a get 503 and counts one failed in `_refresh`'s `_shards`.
- AC-8: `Shard::get` and the lookup it uses: the shard-level cases of AC-1 and AC-2, over a shard that was refreshed, one that never was, and one whose newest write is a delete.

## Test plan

`crates/node/tests/get_route.rs` over the emulator and a real server socket, as `doc_route.rs`; `crates/writer/tests/shard_get.rs` for AC-8.

## S3 request budget

None: a get and a refresh read and commit the shard's in-memory directory. They add no blob request.

## Switch

None: the behaviour is stock's.

## Risks

- A realtime get refreshes the shard, which commits its in-memory directory; a get-heavy workload on a hot id commits per get. Stock does the same (`refresh("get")`); the next refresh the publish timer makes is as cheap as before.
