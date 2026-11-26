# Prometheus metrics: verification

Run in the cloud sandbox with `cargo test -p oss-node --features reader,writer,coordinator`. The tests failed to compile before the route existed, then passed.

- AC-1: `the_body_is_prometheus_text_with_the_ledger_s_totals` (node, `tests/metrics_route.rs`; its helper checks the HELP, TYPE and sample grammar of every line).
- AC-2: `the_body_is_prometheus_text_with_the_ledger_s_totals` (node, `tests/metrics_route.rs`): requests by op, bytes and cost units against a known PUT.
- AC-3: `a_held_reservation_raises_its_pool_s_in_use_only` (node, `tests/metrics_route.rs`).
- AC-4: `a_gate_that_is_on_is_reported_as_gauges_and_counters` and `the_search_gate_and_queue_families_appear_only_when_they_are_on` (node, `tests/metrics_route.rs`). NOT-RUN: the queue families with the queue on, no test builds one.
- AC-5: `a_node_without_store_usage_serves_no_usage_route` and `a_node_with_no_store_usage_setting_serves_the_usage_route` (node, `tests/server/boot_usage.rs`). NOT-RUN: a non-admin request to the route; it is in the admin-only table.
- AC-6: `a_scrape_makes_no_blob_request` (node, `tests/metrics_route.rs`).
