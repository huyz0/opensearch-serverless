# The engine stores a document's custom routing and a get returns it

## Serves

A get returned no `_routing` because the engine never stored the routing a document was indexed with, and a forwarded get always answered `routing: None`. Builds on [node-get-realtime](../node-get-realtime/SPEC.md).

## Change

- `oss-doc`: `MetaField::Routing` (`_routing`, one term, omit norms, stored string) and `Indexer::document_routed`, which adds it after `_id` for a document indexed with a routing value. `document` is `document_routed` with none.
- `oss-writer`: `Shard::index_routed` takes the routing; `index` is it with none. A routed document takes the complete path, not the fast ingest plan. `replay` keeps the routing a record carries. `Fetched.routing` is read from the stored field.
- `oss-node`: the bulk path passes the action's routing to the engine; a forwarded get answers it; the REST get writes `"_routing"` after `_primary_term` and before `found`.

## Acceptance criteria

- AC-1: a document indexed with a routing returns it from a realtime get and, after a refresh, from the segment; one indexed without returns none, and a newer version without routing clears it.
- AC-2: a replayed record keeps its routing.
- AC-3: the REST get returns `_routing` for a routed document and no `_routing` key for another.
- AC-4: a forwarded get returns the routing.

## Test plan

`crates/writer/tests/shard_get.rs`, `crates/node/tests/get_route.rs`, `crates/node/tests/read_forward.rs`.

## S3 request budget

None. LIST 0, HEAD 0, PUT 0.

## Risks

- Routed documents skip the fast ingest plan: slower per document, and only for documents that name a routing value.
- Not here: a search over `_routing`, and the reader overlay storing it.
