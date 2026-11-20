# Verified: a store call waits for an I/O buffer

- AC-1: `a_call_waits_for_a_buffer_when_told_to_and_is_busy_after_the_wait` in crates/blob/src/pooled.rs
- AC-2: `a_call_waits_for_a_buffer_when_told_to_and_is_busy_after_the_wait` in crates/blob/src/pooled.rs
- AC-3: `no_free_buffer_is_busy_and_sends_nothing` in crates/blob/src/pooled.rs
- AC-4: `a_search_from_any_node_finds_every_document` in crates/node/tests/cluster_suite.rs
