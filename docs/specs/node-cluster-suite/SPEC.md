# A multi-node cluster suite

## Serves

Parity plan R7: three real nodes on one store, driven through REST only, so that what the pieces do together is tested and the gaps between them are found.

## Change

- `crates/node/tests/cluster_suite.rs` with its rig `tests/suite_rig/mod.rs`: three nodes started by `boot::start` over an S3 HTTP front end on the in-process emulator with the request latency of the Java shell's test kit (GET 20-30 ms, PUT 40-60 ms, LIST 40-70 ms, here divided by four to keep a run short), or over a RustFS process when `RUSTFS_BIN` names one.
- `Node::crash` stops a node as a killed process stops: servers and renewal end, the lease is not released and lapses.
- `ClusterRoutes` no longer reads a lease's stamp: the membership view keeps a lease's copy until it runs out, so a live node's copy passes its stamp for up to a renewal, and judging by it moved shards between nodes whenever that happened (found by this suite; [node cluster routing](../node-cluster-routing/SPEC.md) AC-8). `ClusterRoutes::new` loses its clock.
- Not here: the gaps listed under Risks.

## Acceptance criteria

- AC-1: a bulk with `refresh=true` through each of three nodes is applied: every item is created, though the shards are owned by different nodes.
- AC-2: a get through each of the three nodes then finds each document: a node that does not hold the shard asks the one that does ([forwarded reads](../node-read-forward/SPEC.md)).
- AC-3: a search from each node finds every document, with every shard answering.
- AC-4: with the first node killed, the others see it go when its lease lapses; writes to every shard then succeed through either survivor, and every document, those the dead node wrote and those written since, is found by a get through either survivor: its shards were taken over with their history.
- AC-5: AC-4's scenario over RustFS, where `RUSTFS_BIN` is set (skipped, and says so, otherwise).

## Test plan

`crates/node/tests/cluster_suite.rs`: one test per criterion. The scenario of AC-4 was first red on the placement defect above (a live node looked dead, its shards moved, and writes to them answered 503 for as long as the old holder lived); AC-8 of node cluster routing pins that.

## S3 request budget

Tests only; no product request changes except that `ClusterRoutes` no longer makes a shard move on a stale stamp, which removes head contention.

## Risks

- #219: a shard whose rendezvous owner changes while another live node holds its head answers 503 for as long as that node lives; the suite settles the members before it creates the index.
- A killed node's lease lapses in one lease lifetime (3 s here) plus a view refresh, so AC-4 waits about that long.
- RustFS was not run in the session that wrote this (no binary); AC-5 is unverified until a run with `RUSTFS_BIN`.
