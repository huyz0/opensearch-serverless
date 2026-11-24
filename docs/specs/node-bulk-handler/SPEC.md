# The owner's answer to a forwarded bulk

## Serves

Parity plan R7.6, cluster routing: the receiving half of [forward](../forward/SPEC.md) and [node-cluster-routing](../node-cluster-routing/SPEC.md). A node that is not the owner sends a shard's slice of a bulk to the owner as `internal:serverless/document/bulk`; this is what the owner runs. Invariants: S3 requests are the bill (the handler adds none of its own; the group is staged in the node's one write log as a local group is); fencing is by the shard head's conditional write, never a lock ([principles](../../principles.md#write-path)).

## Change

`oss-node` (`writer` feature) gains `bulk_handler::BulkHandler`, a transport `Handler` over the `WriterRole`:

- It decodes the `BulkRequest`, finds the index by name through `Routes::index`, checks the uuid the sender knew (when it sent one) and that the shard number is in range, rebuilds each operation (a delete, an index, or an index that requires an absent id, with its `if_seq_no`, `if_primary_term` and routing) and applies the group with `WriterRole::apply_group`, which opens the shard through the node's opener when it is not held.
- It answers one `BulkOutcome` per operation in order: created, updated, deleted or not found with version, sequence number and term; a conflict (409) and a retryable failure (429) flagged as the sender reads them; any other failure with its reason.
- When the request asks for a refresh, it refreshes the shard once after the group.
- A request it cannot decode, an unknown index, a uuid that is not the index's, or a shard out of range is refused with a typed remote exception and nothing is applied.
- Not here: registering it on a server (the boot spec), update actions, ingest pipelines.

## Acceptance criteria

- AC-1: a forwarded group of an index, a create, a delete and an index with `if_seq_no` is applied to the shard in order, and each outcome reports the written version, sequence number, primary term, `created` and `found` as the engine gave them.
- AC-2: a lost condition answers `conflict`, and the other items of the group are still applied.
- AC-3: an unknown index, a uuid that is not the index's, a shard number out of range and a message that does not decode are refused with a remote exception, and the shard is untouched.
- AC-4: a request with `refresh` answers `refreshed`, and the documents are visible to a realtime get afterwards.
- AC-5: a group for a shard this node does not hold and cannot open answers a failure for each item, not a refusal of the request.

## Test plan

`crates/node/tests/roles/bulk_handler.rs` calls the handler directly over a writer role on the emulator, as `assemble.rs` builds one; each test run red before the code.

## S3 request budget

None of its own: the group is one commit to the write log, as a local group is.

## Bounds

The request is already bounded by the transport frame limit; its operations are copied once into the engine's inputs (one `String` and one `Vec<u8>` an operation).
