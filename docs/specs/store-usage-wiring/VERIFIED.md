# Store usage wiring: verification

- AC-1: `store_usage_is_on_by_default_off_when_set_and_bounded` (node, `boot/config/tests.rs`).
- AC-2: `a_node_with_store_usage_on_charges_an_index_and_the_node` (node, `tests/server/boot_usage.rs`).
- AC-3: `a_node_without_store_usage_serves_no_usage_route` (node, `tests/server/boot_usage.rs`) and `a_node_with_no_store_usage_setting_serves_the_usage_route` (the default).
- AC-4: `the_route_lists_every_kind_and_writes_thousandths_as_decimals` (node, `tests/usage_route.rs`).
