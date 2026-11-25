# Verified: resident page bound

- AC-1: `the_resident_page_bound_is_off_by_default_and_refuses_what_it_cannot_hold` in crates/node/src/boot/config/tests/resident.rs (default, text, under 1 MiB); `a_resident_bound_over_the_page_cache_pool_is_refused` in crates/node/src/page_cache_disk.rs (over the pool)
- AC-2: `a_bound_holds_resident_pages_and_forces_refaults_and_answers_are_unchanged` in crates/node/tests/resident_pages.rs (the pager's budget after boot); seen red with the apply line removed (budget stayed 1073741824)
- AC-3: `a_bound_holds_resident_pages_and_forces_refaults_and_answers_are_unchanged` in crates/node/tests/resident_pages.rs: with the setting 0, 300 searches over a 10.4 MB index left 9.2 MB resident and 0 evictions; with 1 MiB, evictions and refaults grew and resident bytes stayed within 1.25 MiB
- AC-4: `a_bound_holds_resident_pages_and_forces_refaults_and_answers_are_unchanged` in crates/node/tests/resident_pages.rs: the hits of all 300 searches equal with and without the bound
- AC-5: `a_bound_holds_resident_pages_and_forces_refaults_and_answers_are_unchanged` in crates/node/tests/resident_pages.rs: the usage report's resident-pages section is absent with the setting 0 and present with it set

Finding behind the spec: lucene-rust's pager already bounds resident pages (`Pager::set_budget`, default 1 GiB, process-wide); the node never set it. No lucene-rust change.

Measured here (provisional: fs store, no request latency, one cloud sandbox): resident 9,175,040 B after the unbounded run. NOT-RUN: `scripts/gate.sh`, coverage, mutants; the cold-cache search-lane measurement (docs/specs/search-slots) on a store with latency.
