# The tail's four transport links

## Serves

Parity plan R7.6, the tail overlay engine, seventh piece ([ADR 0018](../../adr/0018-tail-overlay-in-the-reader.md), Accepted; [ADR 0016](../../adr/0016-native-transport-and-early-revoke.md)). Gives [writer-tail-fetch](../writer-tail-fetch/SPEC.md), [writer-tail-push](../writer-tail-push/SPEC.md) and [reader-tail-held](../reader-tail-held/SPEC.md) their transport, as [writer-push-link](../writer-push-link/SPEC.md) did for the commit push. Fetch on open and the node's wiring are later pieces.

## Change

`oss-node`:

- `tail_link` (`writer` feature): `TransportTailLink`, a `TailLink` that sends a tail push (`internal:serverless/tail/push`) to the reader a lease names, signed, waiting at most 500 ms, and gives its answer; and `TailFetchHandler`, the transport handler for `internal:serverless/tail/fetch` over `TailServe::answer`.
- `tail_reader_link` (`reader` feature): `TailPushHandler`, the handler for a tail push over `TailHeld::handle`; and `TransportTailFetch`, which asks the writer of a shard, through `LiveWriters`, for the records past a commit's checkpoint and gives its answer.

A writer the leases do not show live is asked nothing. Nothing here reads the object store, and nothing runs until a node registers the handlers and a caller asks.

## Acceptance criteria

- AC-1: a tail push sent through the link reaches a reader's guarded handler with every record as sent and the answer names the shards the reader does not want; the shard a reader tracks is held, one it does not is named unwanted.
- AC-2: a push the reader refuses (another secret), to a node the leases do not name, to a revoked lease, to an address that is none, to one nobody listens on, to a process other than the one the lease names, or a push over 8 MiB is a link error, and the reader holds nothing.
- AC-3: a fetch asked of a live writer over a real socket, signed, reaches the writer's handler with every field as sent and comes back with the records above the checkpoint, a subscription and its grant; fed to `TailHeld::answered` the shard is then granted.
- AC-4: a fetch to a writer the leases do not show live, one that does not answer in the timeout, one that fails, or one that answers bytes that do not decode gives nothing.
- AC-5: each handler refuses a message that does not decode or is over its bound (8 MiB a push, 4 KiB a fetch) and answers a fetch or push for a shard it knows nothing about as the engine does (no grant; unwanted).

## Test plan

`crates/node/tests/tail_links.rs`: real servers on loopback with guarded handlers, a real `TailKeep`/`TailServe` on the writer and `TailHeld` on the reader.

## S3 request budget

None: the links go between nodes and read the leases the node holds.

## Switch

None of their own: a link is inert until a node registers a handler and the overlay's setting (`serverless.search.tail_overlay.enabled`, default off) has a reader ask.

## Risks

A lease address is a hint; a wrong one is a lost push or fetch, and the reader serves the commit, which is never older than the register.
