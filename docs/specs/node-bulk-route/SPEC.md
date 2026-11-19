# A writer node serves `_bulk`

## Serves

Parity plan R7.6, node assembly: the HTTP route that takes a `_bulk` body, streams it through the bulk parser in slices charged to the bulk budget, applies each to this node's writer role, and answers the items. Index creation, the ingest pipeline and forwarding to another node are separate pieces (`Routes` already forwards).

## Change

`oss-node`, with the `writer` feature, gains `bulk_route`: `BulkService` and `routes`.

## Acceptance criteria

- AC-1: `POST` and `PUT` of `/_bulk` and of `/{index}/_bulk` apply every item to a shard held here and answer 200 with `took`, `errors` and one result a item, in order, the path's index being the default of an item without one.
- AC-2: an item the engine refuses is reported as failed in its own result, `errors` is true, and the other items are still applied.
- AC-3: `?refresh=true` (or empty, or `wait_for`) refreshes the shards written before the answer, `false` or none does not, and any other value is 400 `illegal_argument_exception`.
- AC-4: a body the bulk parser refuses is answered with the parser's status and words, and an index the node has no route for is a failed item, not a failed request.
- AC-5: a body larger than one slice is applied slice by slice: every item of a body of several slices is applied and the budget is free again after the answer.

## Test plan

`crates/node/tests/bulk_route.rs` over the emulator and a real server socket.

## S3 request budget

The writes stage in the node's one log as for any bulk; the route adds no request.
