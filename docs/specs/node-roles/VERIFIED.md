# Node roles: evidence

All in `crates/node/tests/server/boot_roles.rs` (`cargo test -p oss-node --all-features --test server boot_roles`, 4 passed).

- AC-1: `the_variable_names_the_roles_and_refuses_what_it_does_not_know`.
- AC-2: `a_booted_nodes_lease_names_its_roles` (leases read from the register).
- AC-3: `a_search_node_hands_its_writes_to_the_ingest_node_and_serves_what_they_wrote` (the shard head names the ingest node). The forwarding already existed, so this test did not fail when the opener was left in; it holds the behaviour.
- AC-4: `a_search_node_alone_refuses_a_write_and_holds_no_shard`. Planted: with the ingest check always true the test fails (the write was applied locally, a head exists).
- AC-5: `a_search_node_hands_its_writes_to_the_ingest_node_and_serves_what_they_wrote` (get and search through the search node answer).
- AC-6: `a_search_node_hands_its_writes_to_the_ingest_node_and_serves_what_they_wrote` (the reader placement over the two leases names only the search node as a reader, and the writer placement only the ingest node).
