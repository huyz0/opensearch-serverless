# Bulk: verification

- AC-1: `bulk_steps_answer_as_stock_opensearch_does` (writer, `tests/bulk_fixtures.rs`, over `fixtures/compat/bulk/*.json`, each fixture replayed with the whole body in one slice and with one operation a slice); `refusals_carry_the_javas_messages` (writer, `tests/bulk_parse.rs`).
- AC-2: `a_64_mib_bulk_streams_in_slices` (writer, `tests/bulk_stream.rs`, counting allocator: peak below 6 MiB for a 64 MiB body).
- AC-3: `grants_come_in_arrival_order`, `an_oversized_request_gets_the_whole_budget`, `a_slice_that_waits_past_the_timeout_is_refused_with_429` and `holders_are_bounded` (writer, `src/bulk/budget.rs`).
- AC-4: `bulk_steps_answer_as_stock_opensearch_does` (the per-shard sequence numbers of `fixtures/compat/bulk/shards.json`); `routing_shards_scale_the_hash_as_core_does` (cluster, `tests/routing/hashes.rs`).
- AC-5: `a_foreign_group_is_forwarded_once_and_answers_in_request_order` and `each_slice_forwards_its_own_groups` (writer, `tests/bulk_forward.rs`).
