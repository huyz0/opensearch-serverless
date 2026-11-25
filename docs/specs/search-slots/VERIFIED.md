# Verified: search slots

- AC-1: `a_permit_is_taken_and_given_back_and_the_tenant_entry_goes` in crates/node/tests/search_slots.rs
- AC-2: `a_gate_that_is_not_full_admits_at_once_and_a_full_one_lets_a_tenant_within_its_share_run` in crates/node/tests/search_slots.rs
- AC-3: `a_tenant_over_its_share_waits_on_a_full_gate_and_a_quiet_one_does_not` in crates/node/tests/search_slots.rs
- AC-4: `a_wait_ends_at_its_deadline_and_the_search_runs_as_an_overflow_runner` in crates/node/tests/search_slots.rs
- AC-5: `past_the_waiter_cap_an_over_share_search_runs_at_once` in crates/node/tests/search_slots.rs
- AC-6: `a_level_two_tenant_waits_at_half_the_slots_only_with_another_tenant_active` in crates/node/tests/search_slots.rs
- AC-7: `a_tenant_over_its_share_waits_on_a_full_gate_and_a_quiet_one_does_not` in crates/node/tests/search_slots.rs
- AC-8: `the_report_counts_what_happened` in crates/node/tests/search_slots.rs
- AC-9: `the_state_holds_one_entry_a_tenant_and_no_more` in crates/node/tests/search_slots.rs
- AC-10: `the_settings_default_to_off_and_refuse_what_leaves_no_headroom` in crates/node/tests/server/boot_search_slots.rs
- AC-11: `a_node_with_slots_serves_searches_and_reports_the_gate` in crates/node/tests/server/boot_search_slots.rs

## Measurements for the default (provisional)

Run by hand: `cargo test -p oss-node --all-features --test search_slots_measure -- --ignored --nocapture` (crates/node/tests/search_slots_measure.rs). A debug build on 4 cores, a node over a file store, two indexes of 3,000 documents, a bool query with a terms aggregation, closed-loop clients over HTTP, 8 slots, 3 alternating rounds of 6 s each, gate off against gate on. Two full runs:

| Run | Scenario | Gate off | Gate on |
|---|---|---|---|
| 1 | 6 clients, one tenant, below the slots | p50 2.1 ms, p99 4.8 ms | p50 2.1 ms, p99 4.9 ms (+2%) |
| 2 | the same | p50 2.0 ms, p99 4.7 ms | p50 2.1 ms, p99 5.1 ms (+8%) |
| 1 | 60 noisy clients and 2 quiet | quiet p99 43.5 ms, noisy p99 188.8 ms | quiet p99 42.8 ms, noisy p99 206.0 ms |
| 2 | the same | quiet p99 43.7 ms, noisy p99 201.7 ms | quiet p99 42.9 ms, noisy p99 207.1 ms |

What it shows:

- Below the slots, the gate's cost on p99 was 2% in one run and 8% in the other, so it does not hold the 5% bound on this machine; the run to run spread is as large as the difference.
- At saturation, the gate did not lower the quiet tenant's p99 (43.7 to 42.9 ms) and moved the noisy tenant's throughput down by about 10% (26,940 to 24,060 searches). Its counters in run 2: of about 15,000 noisy searches a round, about 600 waited (longest 169 to 350 ms, none past the wait), and about 13,600 ran as overflow runners because the gate was full and the tenant was not over its share while the quiet tenant was between searches.
- The quiet tenant's tail comes from sharing 4 cores with 60 busy threads, which the gate does not limit: it orders who holds a permit, and an overflow runner uses no permit.

Conclusion: the default stays off (`OSS_SEARCH_SLOTS` 0). On this workload the gate gives no protection and a small cost; the case for it is a node where searches hold something other than CPU (a store read, a memory pool) so that holding fewer at once lowers the others' latency. That needs a measurement on a store with request latency, not run here. The soak with searches failing mid-wait, showing every permit returned, is covered by the guard's drop in AC-1 and is not run as a soak.

Cold-cache run on a store with latency (`crates/node/tests/search_slots_cold.rs`, run by hand; provisional, one cloud sandbox, search-only node, `OSS_PAGE_CACHE_BYTES` 2 MiB, 60,000-document noisy index of about 10 MB, 500-document quiet one, every search a different term). With `OSS_RESIDENT_PAGE_BYTES` unset the noisy index fits the 1 GiB pager budget: after warm-up 40 noisy clients run at p50 3 ms with 39 to 55 store GETs in 8 s, so nothing is cold. With the bound at 4 MiB the noisy tenant goes to the store:
- Emulator, Java latency profile at half, 40 noisy and 2 quiet clients, 8 s: noisy p50 1.1 s, about 2,100 GETs; quiet p99 0.9 ms with the gate off and on (the noisy searches wait on the store, not the CPU, so the quiet tenant is not held up).
- Emulator, 200 noisy and 2 quiet clients (the REST workers saturated), 3 rounds of 15 s each: gate off, quiet p50 992 ms, p99 1,849 ms, 18,534 GETs; gate on (8 slots), quiet p50 877 ms, p99 1,930 ms, 18,046 GETs; noisy p99 4,237 ms off, 4,186 ms on.
- RustFS 1.0.6 (SHA-256 of the zip as in the recipe), same load, GETs not metered: gate off, quiet p50 600 ms, p99 915 ms; gate on, quiet p50 632 ms, p99 1,172 ms; noisy p99 2,459 ms off, 2,913 ms on.

Conclusion: the gate does not help even with a cold noisy tenant. With slots held below the workers, overflow runners and waiters still take workers while their reads wait on the store, so a quiet search queues for a worker either way; the differences above are inside the run-to-run spread (28 to 184 quiet samples a run). The default stays off (`OSS_SEARCH_SLOTS` 0). What would change it is a lane that bounds the store reads in flight per tenant, not the searches admitted. The 4 MiB bound is also shared by the quiet index: the pager's budget is process-wide, so the noisy tenant evicts the quiet one's pages as well.
