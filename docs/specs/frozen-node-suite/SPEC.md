# Frozen-node suite

## Serves

Roadmap R8.6 (takeover fault suites: "no frozen-node suite"); [ADR 0021](../../adr/0021-proactive-takeover-from-the-node-log.md) and the principle that a paused or partitioned writer is already safe because fencing is a conditional create, never a lease ([principles](../../principles.md#write-path)). After the overlay takeover fault tests, which kills the owner.

## Change

Tests and rig only; no product code. A killed node is the easy fault: its requests stop. A frozen node (a long GC pause, a stopped VM, a process under SIGSTOP) is the hard one: its requests are in flight, its lease runs out while it cannot renew, a successor takes the shard, and then its late requests arrive.

The rig (`tests/suite_rig`) gains a `Freezer`, a TCP proxy a node's object-store traffic goes through. Frozen, it holds every byte in both directions without closing a connection, so the node sees a stall, not an error; thawed, it releases them in order. A node is started behind one with `Cluster::add_behind`, and `Cluster::freeze` and `thaw` act on its slot.

What the proxy does not do: the node's own threads and timers keep running while frozen (a stopped process would stop them too), so a frozen node still notices time passing, its requests simply do not complete. The suite holds a node's store traffic for longer than the lease TTL and less than the S3 client's I/O timeout (30 s), so the stall is never turned into an error. A real-process SIGSTOP case is not here: add one only if it is cheap.

A survivor does not forward a write to the frozen owner until the owner is out of its lease view: a write forwarded to a node whose store traffic is held waits the 30 s forward timeout. The suite therefore writes through a survivor two TTLs after the freeze.

## Acceptance criteria

- AC-1: the freezer holds a request and its response while frozen and delivers both, in order and whole, when thawed; a connection opened while frozen is served after the thaw; the held bytes are bounded by one chunk a connection direction.
- AC-2: with the shard's owner frozen past the lease TTL, the other nodes see it go and a write through a survivor takes the shard and is acknowledged.
- AC-3: a write sent to the frozen owner while it is frozen is not acknowledged before the thaw, and after the thaw is refused (its append finds the slot fenced) and counted by no node.
- AC-4: after the thaw every document acknowledged before and during the freeze is counted exactly once by every node (no loss, no double), and the thawed node serves a write for the shard by passing it to the owner, which writes it once.
- AC-5: the thawed owner's late log append does not land in a slot the successor fenced: no document appears that no client was told was written, and the shard's head is held by one node.

## Test plan

- AC-1: a unit test of the freezer against a plain TCP echo server (`tests/suite_rig/freeze.rs`'s own tests, run by `tests/frozen_node.rs`).
- AC-2 to AC-5: `tests/frozen_node.rs` over a three-node cluster with a 3 s lease TTL and the owner of the shard behind a freezer, the same shape as `overlay_takeover.rs`.

## S3 request budget

Unchanged: no product code. The suite counts nothing new.

## Switch

None.

## Risks

A proxy holds bytes; a frozen node whose held request is longer than the client's 30 s I/O timeout would see an error instead of a stall, so the freeze is bounded below that. Three clusters on one machine starve each other's renewals, so the suite runs one cluster at a time as the other cluster suites do. A pass here says nothing about a process that is truly stopped (its timers stop too).
