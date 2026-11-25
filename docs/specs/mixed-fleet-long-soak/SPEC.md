# Mixed Java/Rust fleet: a long hand-over soak

## Serves

Parity plan R15, the Java-retire gate. [mixed-fleet-soak](../mixed-fleet-soak/SPEC.md) proves a few hand-overs between live Java and Rust nodes. This repeats them for as long as a budget allows, with a standby of the other kind always running, and judges every acknowledged write at every step.

## Change

A test, `crates/node/tests/mixed_fleet/soak.rs` (a module of `mixed_fleet.rs`, ignored unless `RUSTFS_BIN`, `OSS_JAVA_CP` and `OSS_JAVA_BIN` are set), on one RustFS bucket. A Java node and a Rust node run, the owner is killed (SIGKILL) and the other kind takes the shard over; a fresh node of the killed kind then starts as the next standby, and the next cycle kills the new owner. Cycles alternate the killed kind. `OSS_SOAK_CYCLES` (default 6, at most 98: the ledger must fit one match-all of 10,000 hits) and `OSS_SOAK_MINUTES` (default 0, no time limit) bound a run; it stops at the first of the two. The index has two shards.

Each cycle: write a batch of 100 documents to the owner; kill it; write a batch to the standby, which takes over; start a replacement of the killed kind; judge the ledger on both live nodes. The test prints one line a cycle (kind killed, time to the first acknowledgement after the kill, store requests by kind) and a summary (cycles, acknowledged documents, the fastest, median and slowest take-over by kind).

## Acceptance criteria

- AC-1: over every cycle no acknowledged document is missing and none is found twice, on either live node, after each hand-over (a run that loses one fails and names its id).
- AC-2: the first acknowledgement after each kill comes within the lease TTL plus the bound of [mixed-fleet-soak](../mixed-fleet-soak/SPEC.md) AC-6; a take-over that does not fails the run.
- AC-3: the slowest take-over of a run, by kind, is printed with the median, so a drift across cycles shows; the run fails if the slowest of the last third of the cycles is more than twice the slowest of the first third plus the TTL.
- AC-4: on the emulator (no `RUSTFS_BIN`), the run prints the store's LIST count over the cycles (the nodes' start-ups and the writes), as a cycle's mean; RustFS has no meter, so there it is not counted. It is a number to read, not a bound.

## S3 request budget

None added: the test only observes and adds the batches it writes.

## Switch

None. A test.

## Risks

- A cycle takes about 7 s at a 4 s lease on RustFS (a take-over of 4 to 6.6 s, then the replacement's start), so 98 cycles take about 12 minutes; the container's budget, not the test, bounds a run.
- The RustFS process and Java nodes share the machine with the Rust nodes; times are relative, never absolute.
