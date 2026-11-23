# The coordinator routes each bulk item to a shard and groups items by shard

## Serves

Parity plan R7.3, document APIs, third piece: the Java shell's `BulkDispatch.route` and the grouping step of `BulkDispatch.dispatch` (documents package, shell 429b4cbc). The per-group write, throttling, ingest pipelines and the response are the node's and later pieces.

## Change

`oss-coordinator` gains `docs::bulk`: `route_items`, which resolves each item of a bulk request to the backing index, its uuid and its shard, or to the item's own failure; and `group_by_shard`, which collects the routed items into one group for each `index[shard]`, in the order first seen.

## Acceptance criteria

- AC-1: an item already failed is left as it is and its index is not looked up.
- AC-2: an item with no index, or a blank one, fails 400 `missing_index`: "no index given for this item and none in the path".
- AC-3: an item naming a system index fails 403 `system_index`: "[i] belongs to a plugin and is not reachable through the request path", before any lookup.
- AC-4: a lookup that cannot be read fails every item of that index 503 `metadata_unavailable`: "could not resolve index [i]; retry"; an index that does not exist fails 404 `index_not_found`: "no such index: i"; each name is looked up once however many items name it.
- AC-5: an item addressed at a data stream that is not a create fails 400 `illegal_argument_exception`: "only write ops with an op_type of create are allowed in data streams; address the backing index [b] for anything else".
- AC-6: a routed item reports the backing index it lands on, that index's uuid, and the shard its id (or routing) hashes to; a mapping that requires routing, with none given, fails the item 400 `routing_missing_exception` with the plane's message.
- AC-7: `group_by_shard` gives one group for each `index[shard]`, groups in the order first seen and items in item order within a group; items not routed are in no group.

## Test plan

`crates/coordinator/tests/server/bulk_route.rs`: an in-memory plane counting lookups; statuses, kinds and words asserted exactly.

## S3 request budget

One descriptor read for each distinct index in the request, never for each item.

## Bounds

One memo entry for each distinct index and one routed record for each item; a request's items are already bounded by its memory budget.

## Switch

None: the shell's routing has no setting and no measured downside.

## Risks

The ingest pipeline resolution the shell does in `route` is a separate piece: it needs the backing index's settings, which this piece does not carry.
