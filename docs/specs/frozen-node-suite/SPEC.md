# Frozen-node suite

## Serves

Roadmap R8.6 (takeover fault suites: "no frozen-node suite"); [ADR 0021](../../adr/0021-proactive-takeover-from-the-node-log.md) and the principle that a paused or partitioned writer is already safe because fencing is a conditional create, never a lease ([principles](../../principles.md#write-path)). After the overlay takeover fault tests, which kills the owner.

## Change

Tests and rig only; no product code. A killed node is the easy fault: its requests stop. A frozen node (a long GC pause, a stopped VM, a process under SIGSTOP) is the hard one: its requests are in flight, its lease runs out while it cannot renew, a successor takes the shard, and then its late requests arrive.

The rig (`tests/suite_rig`) gains a `Freezer`, a TCP proxy a node's object-store traffic goes through. Frozen, it holds every byte in both directions without closing a connection, so the node sees a stall, not an error; thawed, it releases them in order. A node is started behind one with `Cluster::add_behind`, and `Cluster::freeze` and `thaw` act on its slot.

What the proxy does not do: the node's own threads and timers keep running while frozen (a stopped process would stop them too), so a frozen node still notices time passing, its requests simply do not complete. The suite holds a node's store traffic for longer than the lease TTL and less than the S3 client's I/O timeout (30 s), so the stall is never turned into an error. The real-process case is `tests/frozen_process.rs`: the node under test is the `oss-node` binary as its own process, stopped with `kill -STOP` and resumed with `kill -CONT`, so its threads and timers stop too.

A survivor does not forward a write to the frozen owner until the owner is out of its lease view: a write forwarded to a node whose store traffic is held waits the 30 s forward timeout. The suite therefore writes through a survivor two TTLs after the freeze.

## Acceptance criteria

- AC-1: the freezer holds a request and its response while frozen and delivers both, in order and whole, when thawed; a connection opened while frozen is served after the thaw; the held bytes are bounded by one chunk a connection direction.
- AC-2: with the shard's owner frozen past the lease TTL, the other nodes see it go and a write through a survivor takes the shard and is acknowledged.
- AC-3: a write sent to the frozen owner while it is frozen is not acknowledged before the thaw, and after the thaw is refused (its append finds the slot fenced) and counted by no node.
- AC-4: after the thaw every document acknowledged before and during the freeze is counted exactly once by every node (no loss, no double), and the thawed node serves a write for the shard by passing it to the owner, which writes it once.
- AC-5: the thawed owner's late log append does not land in a slot the successor fenced: no document appears that no client was told was written. The head is asserted on its own: while the owner is frozen and after its thaw (and after a write through it) the head names the same survivor, never the thawed node.
- AC-6: a node that is a real process stopped with SIGSTOP past the lease TTL is dropped from the survivors' view and a survivor takes the shard (head owner is a survivor); a write sent to it while stopped is held in its socket and, after SIGCONT, is counted by every node exactly once if the reply acknowledged it and by none if the reply refused it; every document acknowledged before and during the stop is counted exactly once by every node; the head still names the survivor after the resume.

## Test plan

- AC-1: a unit test of the freezer against a plain TCP echo server (`tests/suite_rig/freeze.rs`'s own tests, run by `tests/frozen_node.rs`).
- AC-6: `tests/frozen_process.rs`, one `oss-node` process (the owner) and two in-process nodes on one emulated store, 3 s lease TTL.
- AC-2 to AC-5: `tests/frozen_node.rs` over a three-node cluster with a 3 s lease TTL and the owner of the shard behind a freezer, the same shape as `overlay_takeover.rs`.

## S3 request budget

Unchanged: no product code. The suite counts nothing new.

## Switch

None.

## Risks

A proxy holds bytes; a frozen node whose held request is longer than the client's 30 s I/O timeout would see an error instead of a stall, so the freeze is bounded below that. Three clusters on one machine starve each other's renewals, so the suite runs one cluster at a time as the other cluster suites do. A pass here says nothing about a process that is truly stopped (its timers stop too).
