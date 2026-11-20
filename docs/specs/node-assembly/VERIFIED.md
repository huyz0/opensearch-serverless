# Verified: a node assembled from its roles

- AC-1: `every_route_family_is_registered_on_the_one_listener` in crates/node/tests/assemble.rs
- AC-2: `the_role_is_the_one_the_routes_write_through_and_the_timer_publishes_it` in crates/node/tests/assemble.rs
- AC-3: `the_pit_buffer_is_charged_to_the_request_body_pool` in crates/node/tests/assemble.rs
- AC-4: `a_budget_with_no_open_indexes_is_refused` in crates/node/tests/assemble.rs
- AC-5: `the_janitor_timer_sweeps_the_shards_the_node_holds` in crates/node/tests/assemble.rs
- AC-6: `a_node_starts_with_its_janitor_as_a_dry_run` in crates/node/tests/assemble.rs
