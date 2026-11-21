# Verified: node-get-realtime

- AC-1: `a_get_request_that_is_not_realtime_adds_one_trailing_byte_for_a_peer_that_reads_it` in crates/transport/tests/wire/read.rs
- AC-2: `a_get_that_is_not_realtime_does_not_see_an_unrefreshed_write_on_a_peer_of_this_project` in crates/node/tests/read_forward.rs
- AC-3: `a_get_to_a_peer_that_is_not_of_this_project_is_sent_the_java_layout_and_reads_realtime` in crates/node/tests/read_forward.rs
