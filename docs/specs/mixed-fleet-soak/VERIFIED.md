# Verified

RustFS 1.0.6 on localhost (the vendor's prebuilt zip, SHA-256 `a4d109a0…1398f`), Java shell at `feature/serverlessshell`, lease TTL 4 s, 100 documents a batch, two shards. No emulator latency; numbers are provisional.

- AC-1: `a_shard_changes_hands_between_java_and_rust_without_losing_a_write` in crates/node/tests/mixed_fleet.rs, step "AC-1"
- AC-2: `a_shard_changes_hands_between_java_and_rust_without_losing_a_write` in crates/node/tests/mixed_fleet.rs, step "AC-2"
- AC-3: `a_shard_changes_hands_between_java_and_rust_without_losing_a_write` in crates/node/tests/mixed_fleet.rs, step "AC-3"
- AC-4: `a_shard_changes_hands_between_java_and_rust_without_losing_a_write` in crates/node/tests/mixed_fleet.rs, step "AC-4"
- AC-5: `a_shard_changes_hands_between_java_and_rust_without_losing_a_write` in crates/node/tests/mixed_fleet.rs, its ledger judgement; with an id added to the ledger that was never written the run failed with `ledger 201 found 200 (missing [999], unexpected [])`
- AC-6: `a_shard_changes_hands_between_java_and_rust_without_losing_a_write` in crates/node/tests/mixed_fleet.rs, its bounded retry of a batch after a kill: it must be acknowledged within the TTL plus 60 s

Three runs passed (the first with no per-batch times printed). Acknowledging the first batch after a Java kill took 5,375 and 5,927 ms (Rust took over a Java owner), and the batch after a Rust kill 4,053 and 4,881 ms (Java took over a Rust owner), against the 4,000 ms lease; the first batch to a live Java node took about 1.7 s, including opening the shard. 

Run it:

```bash
# the Java shell: see fixtures/mixed-fleet/README.md
RUSTFS_BIN=<rustfs> OSS_JAVA_BIN=$(which java) OSS_JAVA_CP=<classpath> \
  cargo test -p oss-node --features reader,writer,coordinator --test mixed_fleet -- --ignored --nocapture
```

Found on the way: the Java shell answers a bulk request that carries `Connection: close` with the head of a chunked reply and closes before the body, so the client gets `200` and nothing. The test keeps the connection open. Not fixed here (Java side).

Not covered: a Rust node forwarding to a Java owner over transport (the reads here go to the store; the test sends each request to the node that owns or has taken the shard), writes by both kinds of node at once, and a soak longer than a few hand-overs.

- AC-7: `a_bucketed_index_changes_hands_between_java_and_rust_without_losing_a_write` in crates/node/tests/mixed_fleet.rs; first run failed at "AC-2 rust after taking over" with `engine: corrupted index: file mismatch: object id does not match` (the taken-over member restored no commit and wrote a second segment of the same name), passes after the opener restores the member entry; 100 documents a batch, acknowledgements 1.4 s, 4.9 s and 4.3 s.

- AC-8: `a_plugin_request_through_a_rust_node_is_answered_by_the_java_node` in crates/node/tests/mixed_fleet.rs, against a live Java shell on the emulated cluster (in-process S3 front end, no RustFS); passes. A bulk with `Connection: close` is not used.
- AC-9: `a_plugin_request_through_a_rust_node_is_answered_by_the_java_node` again (`_cluster/health` stays native, count unchanged; POST `/_plugins/_ml/models/_search` proxied).

- AC-10: `a_java_node_takes_over_a_shard_a_rust_node_made_from_nothing` in crates/node/tests/mixed_fleet.rs, and `a_shard_made_from_nothing_commits_the_ids_a_java_engine_opens_on` in crates/writer/tests/it/shard.rs; the live run failed without the fix (the Java node could not open the shard) and passes with it, acknowledgements 1.2 s and 2.2 s on RustFS 1.0.6.
