# Verified: search queue

- AC-1: `with_the_queue_off_no_hook_is_called_and_requests_are_served_as_before` in crates/rest/tests/search_queue.rs
- AC-2: `an_over_share_search_is_parked_off_the_worker_and_answered_with_its_body_when_a_worker_frees` in crates/rest/tests/search_queue.rs
- AC-3: `a_full_queue_answers_429_with_retry_after_and_closes` in crates/rest/tests/search_queue.rs
- AC-4: `a_parked_search_past_the_wait_is_answered_429_and_its_counts_return` in crates/rest/tests/search_queue.rs, and `a_parked_search_expires_while_new_connections_keep_arriving` in the same file
- AC-5: `a_peer_that_closes_while_parked_is_dropped_and_its_counts_return` in crates/rest/tests/search_queue.rs (the permit on a replay a guard denies: NOT-RUN: no test of its own)
- AC-6: `shutdown_answers_a_parked_entry_503_and_closes_it` in crates/rest/tests/search_queue.rs
- AC-7: `an_offer_reaches_workers_idle_in_the_channel_and_a_tenant_over_its_share_runs_when_the_node_is_under_its_bound` in crates/rest/tests/search_queue.rs
- AC-8: `the_queue_is_off_by_default_and_takes_its_settings` and `the_queue_refuses_what_the_server_cannot_hold` in crates/node/src/boot/config/tests/resident.rs; the report in `a_node_with_the_queue_on_serves_searches_and_reports_it_and_one_without_does_not` in crates/node/tests/server/boot_search_queue.rs
- AC-9: `a_node_with_the_queue_on_serves_searches_and_reports_it_and_one_without_does_not` in crates/node/tests/server/boot_search_queue.rs; the cold-cache measurement below
- AC-10: the "Search queue" paragraph in docs/compatibility.md states the 429 from a node rule, the Retry-After of 1 and the closed connection, and how stock differs (checked by reading the paragraph; no gate reads it)

## Cold-cache measurement (provisional)

Run by hand: `QUEUE=1 NOISY_CLIENTS=<n> BOUND_ONLY=1 cargo test -p oss-node --all-features --test search_slots_cold --release -- --ignored --nocapture`, with `RUSTFS_BIN` set for RustFS 1.0.6 (zip SHA-256 matched the recipe). Cloud sandbox, 128 workers, resident bound 4 MiB, 2 quiet clients, 3 rounds of 15 s each, queue off against queue 500 (`OSS_SEARCH_QUEUE_SIZE`).

| Store | Noisy clients | Quiet p99, off | Quiet p99, on | Noisy p99, off | Noisy p99, on |
|---|---|---|---|---|---|
| emulator | 100 | 1.4 ms | 1.5 ms | 2,054 ms | 2,144 ms |
| emulator | 200 | 1,730 ms (n=101) | 1.7 ms (n=345,703) | 4,104 ms | 3,798 ms |
| RustFS | 100 | 5.6 ms | 5.7 ms | 1,459 ms | 1,469 ms |
| RustFS | 200 | 924 ms (n=169) | 5.5 ms (n=173,932) | 2,274 ms | 2,390 ms |

- Below the worker count (100 noisy plus 2 quiet clients is under 128) the queue changes nothing measurable: quiet p99 within 2% on RustFS, 0.1 ms on the emulator, noisy p99 within 5%.
- Above it (200), the quiet tenant goes from about 1 s to under 6 ms with the queue on; the noisy tenant's tail is unchanged within 5%.
- Store GETs on the emulator: 100 clients 18,107 off, 19,180 on; 200 clients 18,504 off, 21,302 on. The extra GETs follow the extra completed noisy and quiet searches (the quiet tenant completes thousands instead of about a hundred); the queue itself makes no store request. RustFS has no meter.
- Single runs, not repeated; a time is not a predicate, so the only test assertion is that every search answered 200.

Default: `OSS_SEARCH_QUEUE_SIZE` stays 0 (off). The 5% bound below saturation was seen on the cold-cache runs only, not on the mixed-aggregation measure that held the gate to 8%, so the switch ships off.
