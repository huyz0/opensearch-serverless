# Invariants

The properties the system must never lose, each linked to the check or test that enforces it. `scripts/check-invariants.py` fails when a row names a test or script that does not exist, or names nothing. A row marked `NOT-YET:` is a stated gap, not a hidden one; it gets real evidence when the code it constrains exists. Rationale: [principles.md](principles.md).

| Id | Invariant | Evidence |
|---|---|---|
| I-1 | Every S3 call goes through `oss-blob`; no other crate depends on an S3 client | `scripts/check-s3-access.py` |
| I-2 | LIST is billed in the PUT class, and every LIST call site is justified | `list_is_priced_like_put`, `list_counts_toward_the_put_class`, `scripts/check-s3-access.py` |
| I-3 | Every S3 attempt is counted by the meter, per request kind | `records_each_kind_separately`, `class_total_saturates` |
| I-4 | An idle index makes zero periodic S3 requests | NOT-YET: needs the reader and the in-process emulator |
| I-5 | A write is acknowledged after exactly one PUT to its node's write log, whatever the number of shards it touches | NOT-YET: needs the writer |
| I-6 | A segment is uploaded as one immutable blob and never rewritten | NOT-YET: needs the writer |
| I-7 | A fenced-out writer cannot overwrite its successor's log slot | NOT-YET: needs the writer |
| I-8 | Crate dependencies point downward; reader and writer are independent | `scripts/check-layering.py` |
| I-9 | No `unsafe` in any crate | `scripts/check-unsafe.py` |
| I-10 | No Rust file over 1,000 lines; over 500 only on the shrinking baseline | `scripts/check-module-size.py` |
| I-11 | Tests never reach real AWS | `scripts/check-test-endpoints.py` |
| I-12 | Each role is compiled in only when its build switch is on | `roles_follow_the_build_switches` |
| I-13 | Every in-scope REST endpoint matches OpenSearch's recorded responses, or its difference is documented | NOT-YET: the harness and recorded fixtures exist and check themselves (`recorded_fixtures_replay_cleanly`); the suite runs against `oss-node` once it serves its first endpoint |
| I-14 | Every Rust source file is traceable to the Java it ports, or marked new with a reason | `scripts/check-port-map.py` |
| I-15 | No request path reads a whole body or blob into memory, or uses an unbounded queue; every request-path allocation states its bound | `clippy.toml`, `scripts/check-alloc.py` |
| I-16 | A request that would exceed a memory pool is refused (429) or held back, never allowed to exhaust memory | `take_up_to_the_limit_then_refuse_one_more_byte`, `reserve_exactly_free_bytes_then_refuse_one_more`, `count_checkouts_succeed_and_one_more_is_refused`, `sixteen_threads_never_exceed_the_buffer_count_or_pool_limits` |
| I-17 | Peak memory under sustained load stays under the node's configured budget | `peak_heap_under_sustained_load_stays_within_the_budget`, `ten_thousand_checkouts_and_reservations_allocate_nothing` |
