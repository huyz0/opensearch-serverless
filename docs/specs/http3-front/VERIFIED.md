# Verified

Numbers are from a debug build in a cloud sandbox, client and server in one process over loopback: provisional, relative only.

- AC-1: `with_no_h3_address_nothing_is_bound_or_charged`; `cargo tree -p oss-node --no-default-features --features reader,writer -e normal` shows none of quinn, h3, tokio or h2 (counted 0), and with edge it shows quinn 0.11.12, h3 0.0.8 and h3-quinn 0.0.10
- AC-2: `answers_match_http1_for_a_set_of_requests`
- AC-3: `a_body_over_the_limit_is_413_like_over_http1_and_the_connection_survives`
- AC-4: `a_stream_past_max_streams_is_rejected_and_the_rest_complete`
- AC-5: `memory_is_charged_refused_when_short_and_released_and_a_certificate_is_required`
- AC-6: `heap_does_not_grow_with_the_bytes_uploaded_and_idle_connections_have_a_price` printed `peak 1437474 bytes above the window's start for 4 x 16 MiB uploaded` and `peak 1121652 bytes above the window's start for 4 x 64 MiB uploaded`: under the 4 MiB bound, and the four times larger upload did not raise it. The clients' own sockets are made before the window opens, because each quinn endpoint holds a receive batch of about 3 MiB, which is the listener's charge, not a per-byte cost
- AC-7: `a_client_that_resets_or_vanishes_mid_upload_frees_its_stream_slot`
- AC-8: `only_a_client_offering_h3_is_served`, `memory_is_charged_refused_when_short_and_released_and_a_certificate_is_required` (a missing, unreadable certificate or key names h3); TLS 1.2 is refused by construction (the QUIC settings offer TLS 1.3 only), not by a test
- AC-9: `authority_is_host_and_the_front_sets_no_forwarded_for`, `bodies_are_bridged_and_a_wrong_length_is_not_served_as_complete`, `an_empty_data_frame_does_not_end_a_body_early` and `a_run_of_empty_data_frames_does_not_end_a_body_early` (the first one found, by a missed mutant, that the front served a body cut short at an empty DATA frame: 3 bytes of 7), `a_bad_length_and_an_oversized_head_are_refused_by_the_front` (400 and 431); the header-level refusals (uppercase names, CR, LF, NUL, `:path` forms, hop-by-hop and `1xx` heads) are the shared translate and upstream code held by their unit tests and the HTTP/2 tests, and are NOT-RUN over an HTTP/3 client
- AC-10: `zero_rtt_is_off`; the test fails when the server's early data size is raised (observed red: `early data must not be possible`)
- AC-11: `an_unvalidated_address_is_sent_a_retry_and_then_served`; observed red with the `Retry` branch removed (retry count 0, expected 1). The case of a client that never answers the `Retry` is NOT-RUN separately: it holds no state by construction, because the `Retry` is sent before any connection object exists
- AC-12: `the_tls_listener_advertises_h3_only_when_it_is_configured`
- AC-13: `an_http3_bulk_and_search_make_the_blob_requests_their_http1_twins_make` (crates/node/tests/server/boot_h3_cost.rs): the same blob requests per request kind for a three-item bulk and a match search, over HTTP/3 and over HTTP/1.1, three runs equal each time, the bulk having written blobs
- AC-14: `a_large_response_streams_and_head_carries_no_body` (8 MiB response)
- AC-15: `connections_past_the_limit_are_refused_and_idle_ones_time_out`, `a_handshake_that_never_finishes_is_dropped` (a UDP relay passes the client's first two datagrams and drops the rest; the one slot is held, then free after handshake_timeout), `idle_connections_cost_less_than_the_overhead_charged_for_each` printed `400 idle connections: 32776483 bytes live, 81941 per connection for both ends, about 40970 for the server's half; charged 65536` and `the front with an h3 listener and no connection holds 3081652 bytes; charged 4194304`. The default per-connection overhead is therefore 64 KiB, and 4 MiB is charged once for the listener. The earlier default of 256 KiB charged six times the measurement
- AC-16: `a_stalled_upload_does_not_stall_a_sibling_stream`
- AC-17: `stream_floods_and_connection_floods_are_survived` (1,000 streams opened and cancelled on one connection, then 24 connections against max_connections 8; the next request is served)
- AC-18: `scripts/check-alloc.py` clean over `crates/edge/src` (run by `scripts/tree-checks.sh`)
- AC-19: `latency_of_a_small_get_over_each_protocol_is_recorded` printed `2000 sequential GETs, one connection each, microseconds p50/p99: http3 1623/2812, http2 over tls 1183/2092, http1.1 54/99`. HTTP/3 costs about 1.4 times HTTP/2 over TLS at the median here, and both cost a loopback hop through the front that HTTP/1.1 does not. A record, not a threshold
- AC-20: NOT-RUN: the sandbox's `curl` 8.5.0 is built with nghttp2 and without HTTP/3 (`curl --version` lists no `HTTP3`); the quinn client in the tests is the interoperability evidence meanwhile, and `curl --http3-only` is to be run where a build with HTTP/3 exists
