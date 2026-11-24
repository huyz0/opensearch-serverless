- AC-1: `a_create_stores_the_settings_the_node_reads` (crates/node/tests/server/index_route/create_settings.rs): PASS.
- AC-2: `a_bad_refresh_interval_is_refused_at_create` (crates/node/tests/server/index_route/create_settings.rs): PASS.
- AC-3: `routing_settings_are_checked_against_the_shard_count` (crates/node/tests/server/index_route/create_settings.rs): PASS.
- AC-4: `other_settings_are_refused_and_named` (crates/node/tests/server/index_route/create_settings.rs): PASS.
- AC-5: `a_templates_settings_apply_and_the_request_wins` (crates/node/tests/server/index_route/create_settings.rs): PASS.
- AC-6: `a_create_stores_the_settings_the_node_reads` (crates/node/tests/server/index_route/create_settings.rs): PASS.
- AC-7: `a_create_writes_the_descriptor_and_acknowledges` and the other create tests in crates/node/tests/server/index_route.rs: PASS (three changed, for settings a create now accepts).

Red first: with `crates/node/src/index_route.rs` as on main, five of the six new tests fail (the sixth, a template key the node does not read, passed before too).
