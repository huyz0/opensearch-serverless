# A bulk `update` item is served

## Serves

Parity plan R7.3: the `update` action of `_bulk` on a writer node, for the body the update API takes ([node-update-route](../node-update-route/SPEC.md)): `doc` merge with `detect_noop`, `doc_as_upsert`, `upsert`, and a Painless `script` with `scripted_upsert`. Until now such an item answered 501.

## Change

- `oss-writer`'s bulk module gains the `Updates` trait (what the node plugs in, as `Ingest` is, since the writer cannot depend on the coordinator that plans an update), `Resolved` and `UpdateFailure`, and `Bulk::with_updates`. Planning an `update` item asks the trait to read the document and decide; the item is then applied as an index, create or delete item that carries the sequence number just read, so it keeps the shard's own conflict check. `ItemResult::Noop` is the answer of an update that changed nothing (`result: noop`, `_shards.total` 0, status 200, the version read).
- Without an `Updates` the item still answers the 501 it did.
- `oss-node` implements `Updates` over the writer role by the same decision as the single-document route (`update_decide`, shared with `update_route`), and `BulkService` plugs it in.
- An item whose `_index` and `_id` an earlier item of the same slice already addressed fails with a 409 `version_conflict_engine_exception` saying so: the read happens when the slice is planned and the earlier item is not yet written, so reading again would answer from stale data. The caller retries the item in a later request.
- `retry_on_conflict` on an item is accepted and has no effect: a conflict is that item's 409.

## Acceptance criteria

- AC-1: A bulk `update` item merges a `doc` into an existing document, answers `updated` with the next version, and its action key stays `update`.
- AC-2: An unchanged merge answers `noop` with the version read and writes nothing; a missing document is a 404 item, or `created` (201) with `doc_as_upsert` or `upsert`.
- AC-3: A script item edits the source, `ctx.op` of `delete` deletes, `noop` writes nothing, and with `native_compile` off the item fails with a 501.
- AC-4: A second item for the same document in one slice fails with a 409 naming the cause, and the first item is written.
- AC-5: The caller's `if_seq_no` and `if_primary_term` on the item are checked on the read (409), and an invalid body (not an object, an unknown key) fails the item with a 400, the others going on.
- AC-6: With no `Updates` plugged in, an update item answers the 501 as before.

## Test plan

`crates/writer/tests/it/bulk_update.rs` for the trait seam (planning, rewriting, noop rendering, the same-document rule) with a fake `Updates`; `crates/node/tests/server/bulk_update.rs` over the emulator and a real server socket for the node's decision.

## S3 request budget

One realtime get and one staged write per item, as a get plus an index; no request of its own, no LIST.

## Switch

Merge updates need none. Script items are off unless `serverless.scripting.painless.native_compile` is on.

## Risks

Items are planned in slice order against the shard's state before the slice is applied (the same-document rule above is the guard); Java applies them one by one. The expectations come from reading `TransportShardBulkAction.executeBulkItemRequest` and `UpdateHelper`, not a recorded run.
