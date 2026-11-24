# Verified

```bash
cargo test -p oss-node --all-features --test server boot_ready_drain
cargo test -p oss-node --all-features --lib lifecycle
```

- AC-1: `a_drain_makes_the_node_not_ready_and_takes_the_search_role_from_its_lease` in crates/node/tests/server/boot_ready_drain.rs (the first GET answers 200 with ready true).
- AC-2: `a_drain_makes_the_node_not_ready_and_takes_the_search_role_from_its_lease` again (the GET answers 503 with state draining).
- AC-3: `a_drain_makes_the_node_not_ready_and_takes_the_search_role_from_its_lease` again (a second drain answers already in under half a second against a one-second setting), and `draining_starts_once` in crates/node/src/lifecycle/tests.rs.
- AC-4: `a_draining_leases_roles_lose_search_and_keep_the_rest` in crates/node/src/lifecycle/tests.rs; and the boot test above reads the roles from the lease register as a peer does: both roles before the drain, ingest alone after. With the stamp's condition forced true that test failed ("the search role never left the lease").
- AC-5: `a_drain_makes_the_node_not_ready_and_takes_the_search_role_from_its_lease` again (after the drain the node creates an index and acknowledges a refreshed write). NOT-RUN: a second running node reading the first's roles; the observer here is a membership read of the store, which is what a peer's refresh is.
- AC-6: `a_drain_wait_over_a_minute_is_refused_at_start` in crates/node/tests/server/boot_ready_drain.rs, and `the_drain_settle_follows_the_lease_ttl_up_to_a_minute` in crates/node/src/lifecycle/tests.rs (TTL 300 gives 200, TTL 300000 gives 60000).
- AC-7: `a_drain_makes_the_node_not_ready_and_takes_the_search_role_from_its_lease` again (the probe answers 503 from another connection before the one-second settle is over; took_millis is at least 1000; the roles are read once, straight after the drain answers).

Not covered: a coordinator ranking readers without the drained node (the lease is what readers_for ranks by, tested there), and a drain under load. Hand-over of warm blocks, warm_on_join and drain on SIGTERM wait for R12.1.
