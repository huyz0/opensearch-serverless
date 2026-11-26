# Verified

Numbers are from a debug build in a cloud sandbox: provisional, relative only.

- AC-1: `cargo tree -p oss-node --no-default-features --features reader,writer -e normal` shows no `tokio` or `h2` (counted 0); with no listener set the node starts no front (`memory_is_charged_at_start_refused_when_short_and_released` shows the front is the only thing that charges); a node built without `edge` and given `OSS_EDGE_H2C_ADDR` refuses to start, by `crates/node/src/boot/edge.rs`. NOT-RUN: a node-level test of "no extra port".
- AC-2: `answers_match_http1_for_a_set_of_requests`
- AC-3: `a_body_over_the_limit_is_refused_413_like_over_http1_and_the_connection_survives`
- AC-4: `a_stream_past_max_streams_is_refused_and_the_rest_complete`
- AC-5: `memory_is_charged_at_start_refused_when_short_and_released`, `inconsistent_settings_are_refused`
- AC-6: `heap_stays_within_what_the_front_charges` printed `peak 719537 bytes above the window's start for 64 MiB uploaded`
- AC-7: `a_client_that_drops_its_connection_mid_upload_frees_the_node` and `a_client_that_sends_goaway_mid_upload_and_leaves_frees_the_node` (oss-edge, `crates/edge/tests/h2c_drops.rs`) open a POST to a handler that reads its body, send 4 KiB, then drop the TCP connection or send GOAWAY and leave; the node's open count rises, then returns to its start within 20 s (observed about 20 ms each, loopback, provisional); a reset stream is covered by `rapid_resets_are_disconnected_and_the_node_serves_the_next_request` (oss-edge, `crates/edge/tests/h2c.rs`)
- AC-8: `an_h2_client_gets_h2_and_an_http1_client_gets_http1_with_the_same_answer`, `a_missing_unreadable_or_mismatched_certificate_refuses_to_start`; TLS 1.1 is refused by construction (rustls offers 1.2 and 1.3 only), not by a test
- AC-9: `authority_reaches_the_handler_as_host`, `authority_wins_and_connection_headers_go`
- AC-10: `rapid_resets_are_disconnected_and_the_node_serves_the_next_request`
- AC-11: `an_http2_bulk_and_search_make_the_blob_requests_their_http1_twins_make` (oss-node, `crates/node/tests/server/boot_h2_cost.rs`): a booted node over the in-process S3 emulator, a bulk and a search over clear-text HTTP/2 make the same blob requests per operation, in `S3Op::ALL` order, as their HTTP/1.1 twins on a second index (written after the code, not seen red); `oss-edge` depends on `oss-mem` and not on `oss-blob`, so it cannot make a blob request itself
- AC-12: `a_large_response_streams_through_a_small_window`, and the HEAD case in `answers_match_http1_for_a_set_of_requests`
- AC-13: `heap_stays_within_what_the_front_charges` printed `400 idle connections: 27191992 bytes live, 67979 per connection` (both ends in one process; charged 131072 each); `connections_past_the_limit_are_closed_and_idle_ones_time_out`, `a_plain_client_to_the_tls_port_is_not_served`
- AC-14: `a_stalled_upload_does_not_stall_a_sibling_stream`
- AC-15: `refusals`, `targets_are_origin_form_only`, `cookies_are_joined_and_bodies_are_framed`, `heads_skip_continue_and_frame_bodies`, `chunks_decode_across_any_split`, `a_length_that_differs_from_the_body_resets_the_stream`, `bodies_are_bridged_with_and_without_a_length`
- AC-16: `authority_reaches_the_handler_as_host`, `a_client_sent_forwarded_for_passes_through_unchanged`
- AC-17: `scripts/check-alloc.py`

## Measured on the running node (provisional)

`OSS_EDGE_H2C_ADDR` and `OSS_EDGE_TLS_ADDR` set, `curl --http2-prior-knowledge` and `curl --http2 --cacert` against `/_cluster/health` answer HTTP/2 200; `curl --http1.1` over the TLS listener answers 1.1 200.

- Idle connection memory: 100 and 200 idle clear-text HTTP/2 connections raised the node's resident set by 17.1 and 18.0 KiB each, with no new thread.
- Latency, closed loop of 8 clients against `/_cluster/health`, 3,200 requests per run, Python `httpx`: direct HTTP/1.1 p50 5.8 ms, p99 12.8 to 17.1 ms; with 200 idle HTTP/2 connections open, HTTP/1.1 p99 12.3 ms against 13.6 ms before; through the front p50 7.4 to 9.1 ms, p99 14.1 to 14.5 ms. The front adds about 1.5 to 3 ms at p50 on a call that takes 6 ms, and leaves the HTTP/1.1 tail alone. Write and search paths were not driven: the build used here serves no index-creation route, so `_bulk` and `_search` p99 are not measured.
