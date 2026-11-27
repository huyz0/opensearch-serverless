# Verified: request cost header

- AC-1: `a_request_that_asks_is_told_what_its_store_requests_cost` in crates/node/tests/server/boot_usage.rs
- AC-2: `a_request_that_asks_is_told_what_its_store_requests_cost` in crates/node/tests/server/boot_usage.rs
- AC-3: `the_value_names_every_kind_the_bytes_the_units_and_the_node` in crates/node/src/cost_header.rs
- AC-4: `a_node_with_store_usage_off_sends_no_cost_header` in crates/node/tests/server/boot_usage.rs
- AC-5: `a_request_that_asks_is_told_what_its_store_requests_cost` in crates/node/tests/server/boot_usage.rs
- AC-6: `the_reads_of_the_prefetch_threads_count_in_the_request_that_asked` in crates/reader/src/prefetch/tests.rs (seen failing with the thread's enter removed)
- AC-7: `the_helper_threads_carry_the_scope_of_the_request_that_asked` in crates/coordinator/tests/it/fanout.rs (seen failing with the helper's enter removed)
