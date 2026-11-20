# The cluster half of the writer's routes

## Serves

Parity plan R7.6, node assembly: `Routes::owner` and `Routes::forward` over the leases a node already holds and the native transport. Invariants: S3 requests are the bill (an owner lookup and a forward read no blob); nodes own nothing, so ownership is advice recomputed by every node from the same leases and the shard head's conditional write is the fence ([principles](../../principles.md#write-path)).

## Change

`oss-node` (`writer` feature) gains `cluster_owner`:

- `IndexSource`: the index half of `Routes` (`index(name)`), which a descriptor-backed source supplies.
- `BulkSend`: the one call a forward makes, implemented by `forward::BulkForwarder`.
- `ClusterRoutes<S, L, B>`: implements `Routes`. `owner` is rendezvous placement over the unrevoked, unexpired members with the `ingest` role, this node included, answering `Owner::Local` when this node wins and `Owner::Node(id)` otherwise; with no such member it answers `Local`. `forward` turns a group into one `BulkRequest` (one call per group, never per item), sends it, and turns each `BulkOutcome` back into an item's result.
- Not here: serving a forwarded bulk (the owner's handler), takeover, and the route's room-aware choice of `writerFor`.

## Acceptance criteria

- AC-1: with this node the rendezvous winner among live ingest members `owner` answers `Local`, and otherwise `Node(winner)`; every node over the same leases names the same winner.
- AC-2: a revoked, expired or non-ingest member is never chosen; with no live ingest member the answer is `Local`.
- AC-3: `forward` sends one `BulkRequest` for the group carrying the index name and uuid, the shard, and per item its id, source, deletion, `if_seq_no`, `if_primary_term`, `require_absent` (a `create`) and routing.
- AC-4: each outcome becomes the item's result in the group's order: a created or updated index as `Written` with its version, sequence number and term; a delete as `Deleted` or `NotFound`; a conflict as 409 `version_conflict_engine_exception`; a retryable failure as 429; any other failure as 500.
- AC-5: when the owner is gone or unknown every item fails 503, when the request is too large every item fails 413, when the answer holds fewer outcomes than items the missing items fail 502, and more outcomes than items fails the whole group 502; no item is reported written without an outcome.
- AC-6: an `update` action, or a source that is not UTF-8 (other than a delete), fails its item alone and is not sent.

## Test plan

`crates/node/tests/cluster_owner.rs` with a fake `BulkSend` that records requests and a fixed lease list; each test red before the code.

## S3 request budget

Zero: leases are read from the membership's held copy, and the forward is a transport call.

## Bounds

One request message per group, built by `BulkForwarder` within `MAX_FORWARD_BYTES`; the group's items are already counted by the bulk budget.

## Risks

- A call that fails after the owner applied the group (a timeout, a reset) reports 503 for every item although some may be written; a retried `create` then answers 409. The failure is retryable and ambiguous, as the shell's forward is.
