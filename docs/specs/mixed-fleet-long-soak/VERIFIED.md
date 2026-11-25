# Verified

RustFS 1.0.6 on localhost (the vendor's prebuilt zip, SHA-256 `a4d109a0…1398f`), the Java shell at `feature/serverlessshell` (e71d4233), lease TTL 4 s, 100 documents a batch, two shards, a 4-core container with the nodes, RustFS and the test on it. No emulator latency; numbers are provisional.

Run: `OSS_SOAK_CYCLES=98 RUSTFS_BIN=<rustfs> OSS_JAVA_BIN=$(which java) OSS_JAVA_CP=<classpath> cargo test -p oss-node --features reader,writer,coordinator --test mixed_fleet live_java -- --ignored --nocapture` (the classpath as `fixtures/mixed-fleet/README.md` says).

- AC-1: `live_java_and_rust_nodes_hand_a_shard_over_for_many_cycles_without_losing_a_write` in crates/node/tests/mixed_fleet/soak.rs, its ledger judgement after every hand-over on the taker and on the replacement: 98 cycles (49 kills of each kind), 9,900 acknowledged documents, none missing and none twice, in 731 s. A first 100-cycle attempt failed at cycle 10 with `ledger 1100 found 1000`: the test's match-all asked for 1,000 hits (batch size times ten), so it saw only the first 1,000; the reader now asks for 10,000 and the run is capped at 98 cycles so the ledger fits. Before that fix, 4-cycle and 9-cycle runs passed.
- AC-2: `live_java_and_rust_nodes_hand_a_shard_over_for_many_cycles_without_losing_a_write` again, the bound the write loop enforces (the lease plus 60 s): every first acknowledgement after a kill came within 6.6 s.
- AC-3: `live_java_and_rust_nodes_hand_a_shard_over_for_many_cycles_without_losing_a_write` again, its comparison of the slowest take-over of the first and last thirds of the run: it did not fail. Java took over in 3.9 s at the fastest, 4.7 s median and 5.6 s slowest (49 take-overs); Rust in 4.3 s, 5.2 s median and 6.6 s slowest (49).
- AC-4: NOT-RUN: RustFS has no meter, so the LIST count is only printed on the emulator, which this run did not use.

Not covered: writes by both kinds at once, a forwarded request from a Rust node to a Java owner, searches under the load of the writes, a cycle count above 98 or a run longer than 12 minutes, three or more live nodes, and a take-over by a node that never saw the shard's commit (the one case of `a_java_node_takes_over_a_shard_a_rust_node_made_from_nothing`).
