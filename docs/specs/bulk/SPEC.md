# Bulk: parse, plan and apply as stock OpenSearch answers it

## Serves

Parity plan R6.3 (roadmap stage 3, [roadmap](../../roadmap.md)): the writer takes `_bulk` bodies and answers them as stock OpenSearch does, streaming, inside a memory budget. Builds on the [shard engine](../shard-engine/SPEC.md) and [indexing](../indexing/SPEC.md); the line splitter is [streaming JSON](../streaming-json/SPEC.md)'s `oss_rest::ndjson`.

## Change

- `oss-writer` gains `bulk` (`crates/writer/src/bulk/`):
  - **parse**: action lines as `BulkRequestParser` reads them (server, OpenSearch 3.10 at `429b4cbc`): the `index`, `create`, `delete` and `update` actions, `_index`, `_id`, `routing`, `op_type`, `version`, `version_type`, `if_seq_no`, `if_primary_term`, `pipeline`, `require_alias`, `retry_on_conflict` and `_source`, with its messages for a malformed line, an unknown action or parameter and a missing trailing newline; `request body is required`; then `BulkRequest.validate`'s numbered message over every item (`index is missing`, `id is missing`, the version checks the shard engine already has, `no requests added`). Lines arrive in parts from `ndjson::Splitter`, as the serverless shell's `BulkBodyParser.parsePrefix` takes them: a whole operation at a time, never the whole body.
  - **budget**: `BulkMemoryBudget`: the bytes the node's streamed batches hold, granted first come, first served, at most as many holders as cores; a slice that does not fit waits (its body is not read, so TCP slows the client) and only one that waited past the timeout fails, with a 429. A request larger than the whole budget gets the whole budget when it reaches the front.
  - **plan**: each item's shard as `OperationRouting.generateShardId` picks it over the shell's rendering of the index (`oss_cluster::routing`): `floorMod(hash, routing shards) / (routing shards / shards)`, with routing shards from `index.number_of_routing_shards` and the shard count when it is unset, as `IndexDescriptor.toIndexMetadata` leaves it; until now `DocumentRouting::Legacy` assumed a factor of 1. Ids are generated as `UUIDs.base64UUID` does for an `index` or `create` without one, and the items grouped by shard in request order, as `BulkDispatch` groups them; a group whose shard another node owns goes through a forwarding trait, one call a group.
  - **apply**: a shard's group through `index::Shard`, each item's outcome or failure in the response's terms; one item's failure (a conflict, a mapping refusal, an empty `_id`) never stops the others.
  - **respond**: `BulkResponse` as stock renders it: `took`, `errors`, and each item's `_index`, `_id`, `_version`, `result`, `forced_refresh`, `_shards`, `_seq_no`, `_primary_term`, `status`, or its `error` with `type`, `reason`, `caused_by`, `index`, `shard` and `index_uuid`.
- `?refresh=true` refreshes the shards a bulk wrote and answers `forced_refresh`.
- Fixtures: `fixtures/compat/bulk/items.json` and `shards.json`, recorded from stock 3.10.
- Not changed: the `update` action (a partial merge reads the current source first; it answers as an item failure until a realtime get exists), ingest pipelines, `require_alias` and write aliases, auto-creating a missing index, `wait_for_active_shards`, `?refresh=wait_for`, and acknowledging through the node write log, which is R6.4's with segment publish. Replicas: `_shards` reports one copy, as an index with no replicas does.

## Acceptance criteria

- AC-1: every step of `fixtures/compat/bulk/*.json` replays through `bulk` over in-process shards: each response's status and body equal the recording after its normalisation rules.
- AC-2: a body is parsed as it streams: a 64 MiB bulk applies with the writer's peak heap below 8 MiB over its shards' own (counting allocator), and a body ending mid-line or mid-operation answers as stock does.
- AC-3: the budget grants in arrival order, never holds more than its capacity or holders at once, grants an oversized request the whole budget, and fails only a slice that waited past its timeout, with a 429.
- AC-4: items reach the shard stock's routing picks: the per-shard sequence numbers of `shards.json` equal the recording, with and without `routing`.
- AC-5: a group another node owns is forwarded once, and its outcomes take their places in request order.

## Test plan

`crates/writer/tests/bulk_fixtures.rs` replays the fixtures (AC-1, AC-4); `crates/writer/tests/bulk_stream.rs` feeds a 64 MiB body in 64 KiB chunks under the counting allocator, and bodies cut at every boundary kind (AC-2); `crates/writer/src/bulk/budget.rs` unit tests with a manual clock (AC-3); `crates/writer/tests/bulk_forward.rs` with a recording forwarder (AC-5).

## S3 request budget

Unchanged: shards index in memory; the node write log append and publish are R6.4's.

## Switch

None: the behaviour is stock's.

## Risks

- A routing formula that differs from stock's puts a document on another shard than a Java writer would, which splits an id across shards: AC-4 compares every fixture item's shard.
- Stock computes a default `index.number_of_routing_shards` at index creation (768 for three shards); the shell does not, and the Rust writer routes as the shell does, so a fixture that spreads ids over shards sets it. An index created by stock and read by the shell would route differently in both, which this keeps rather than changes.
- A budget too small for one large document stalls it: an oversized request gets the whole budget at the front (AC-3).
