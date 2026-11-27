# Virtual-time soak: verified

Slice 1, run in the cloud sandbox (4 cores, debug build, emulator with latency off):
`cargo test -p oss-node --features writer --test virtual_soak -- --nocapture`. Times are provisional.

The run: 21 cycles of 8 virtual hours (7 virtual days), seed `0x5eed`, on one `VirtualClock` shared by every node, lease handle, head store and the collector. Each cycle a live owner writes, renews its lease through `LeaseKeeper::renew` every virtual minute and publishes now and then; it then stops (a kill, or a freeze that thaws after the take-over); the next node takes the shard from the lease (`Takeover::from_leases`), and a `DeadLogPass` collects the dead node's log. Shards merge as shipped. Result: 5,069 acknowledged documents, slowest take-over 30,000 ms of virtual time, 4 thawed owners refused, 21 logs collected, 2.2 s of wall time.

No product code changed: every lifecycle step of this slice already took its clock (`LeaseClock` for membership and heads, a closure for publishing and the dead-log pass). The rig change is `node_with_clock` in `tests/proactive_support/packed.rs`, which `node_with_packs` now calls with the clocks it always used, and a `keeper` field on the rig's `Node`.

- AC-1: `a_week_of_kills_and_freezes_loses_nothing_and_collects_every_dead_log` (crates/node/tests/virtual_soak.rs). Every acknowledged write's sequence number must be the count acknowledged before it, so a lost or doubled document across any take-over fails at once; 21 take-overs, 4 of them with a frozen owner that wrote after the thaw and was refused. Seen red: taking over without the predecessor's log (`member(n, false)`) fails with "no take-over in ten leases". NOT-RUN: a read of the documents themselves (the count is by sequence number), and the read on every live node.
- AC-2: `a_week_of_kills_and_freezes_loses_nothing_and_collects_every_dead_log` again: the first acknowledgement after each stop came within the lease TTL (30,000 ms) of the stop, at a 5 s step; the last third's slowest is checked against twice the first third's plus a lease. Seen red: renewing the dead owner's lease through the take-over fails with "no take-over in ten leases".
- AC-3: `a_week_of_kills_and_freezes_loses_nothing_and_collects_every_dead_log` again: after each cycle the collector, stepped 4 minutes a try, removes every blob of the dead node's log (`nodelog/<node>/`), which the test counts by LIST in the test only. A dry run fails it (seen red: the pass run dry leaves 17 blobs). The live owner is not collected: its next cycle writes through its log. One finding: without merging the commit register grows about 3 KB a take-over and passes the rig's 64 KiB read buffer at the nineteenth, so the rig enables merging as the node ships it.
- AC-4: NOT-RUN: slice 2, the janitor's orphan sweep with sightings.
- AC-5: NOT-RUN: slice 2, point-in-time expiry.
- AC-6: NOT-RUN: slice 2, an idle index over the virtual week.
- AC-7: NOT-RUN: slice 3, the 60 s real-time smoke run.

Not covered by this slice and said in the spec: memory, descriptor and thread drift, real latency and `Instant::now` timeouts, a real store, a mixed fleet, clock skew between nodes, the pack collector.
