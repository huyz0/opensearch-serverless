# Fast source ingest: verification

- AC-1: `log_documents_equal_the_complete_path`, `a_wide_mapping_equals_the_complete_path` and `generated_documents_equal_the_complete_path` (doc, `tests/fast_differential.rs`; the generated corpus is 3,000 documents, 506 of them read by the plan in that run).
- AC-2: `a_wide_mapping_equals_the_complete_path` (the refused documents are asserted never read by the plan) and `generated_documents_equal_the_complete_path` (doc, `tests/fast_differential.rs`).
- AC-3: `rust_documents_index_as_the_java_indexes_them` (writer, `tests/segment_fixtures.rs`), run with the plan in front of `Shard::index`; the fixtures' few documents mostly number fields, so the plan reads little of them, and AC-1 carries the equality.
- AC-4: `dynamic_fields_are_read_by_the_plan_once_the_complete_path_has_mapped_them` (doc, `tests/fast_differential.rs`).
- AC-5: `the_plan_is_compiled_when_fields_or_the_mapping_change_and_not_per_document` (doc, `tests/fast_differential.rs`).
- AC-6: `the_plan_builds_log_documents_at_least_twice_as_fast` (doc, `tests/fast_bench.rs`, ignored, release): 2.4x in the recorded run (complete 0.371 s, plan 0.153 s for 50,000 documents), provisional.
- AC-7: NOT-RUN: the comparison needs the plan switched off, so it is a measurement by hand through `Shard::index` on 100,000 log documents (18.7 MB), in-memory directory, release, best of three, provisional: complete path 1.601 s (11.7 MB/s, p50 about 12 us, p99 35 to 46 us a call); plan in front 0.929 s (20.1 MB/s, p50 about 5 us, p99 22 to 34 us a call), 1.7x, and the p99 is lower.
- AC-8: `a_replayed_record_is_a_live_document_with_its_own_identity_and_the_next_write_follows_it` (writer, `tests/shard_replay.rs`) and the rest of `cargo test -p oss-writer`, all passing with the plan in front of replay.
- AC-9: NOT-RUN: building a document touches no blob; the change is in memory only, and the writer's request-meter tests pass unchanged.
