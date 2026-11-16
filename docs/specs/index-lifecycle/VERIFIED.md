# Index lifecycle: verified

Run on 2026-11-16 in a Linux sandbox:
- `cargo test -p oss-meta`, `cargo test -p oss-format`, `cargo test -p oss-blob --all-features`
- the whole gate, `scripts/gate.sh`

An independent review found the drain's cost bounds unasserted, the intent-order test blind to a reorder, and no-op heads and pins that would fail open beside Java nodes. Tests now hold each bound, and the crate ships no no-op heads or pins.

Each planted defect was seen to fail:

| Planted defect | Test that failed |
|---|---|
| a reclaim that drops every intent naming a live name, whatever its uuid | `a_reclaim_removes_only_the_dead_incarnation` |
| a drain that walks on past the first bucket not yet due | `a_drain_lists_nothing_past_the_first_bucket_not_yet_due` |
| a drain that relists from the start after each page | `a_drain_moves_past_a_thousand_kept_intents`, `a_page_ending_in_another_nodes_bucket_skips_the_rest_of_it` |
| a drain that does not skip another node's bucket | `a_page_ending_in_another_nodes_bucket_skips_the_rest_of_it` |
| a retried create that takes any index of the name as its own | `a_retried_create_answers_its_own_record_and_no_other` |
| `had_writer` false for anything but `true` | `had_writer_is_false_only_for_the_literal_false` |
| no intent filed | `a_conditional_delete_files_its_intent_first_then_removes_everything` and four more |
| the intent filed after the descriptor delete | `the_intent_is_written_before_the_descriptor_goes`, `a_delete_that_loses_a_race_leaves_only_an_intent_the_reclaim_drops` |
| pins ignored | `pinned_shards_keep_their_bytes_and_unreadable_pins_purge_nothing` |

Evidence for each criterion:

- AC-1: `a_create_writes_the_descriptor_in_one_request`, `a_retried_create_answers_its_own_record_and_no_other`, `a_create_refuses_a_bad_name_or_unusable_layout_before_any_request`
- AC-2: `a_conditional_delete_files_its_intent_first_then_removes_everything`, `the_intent_is_written_before_the_descriptor_goes`, `a_delete_detaches_the_index_from_its_aliases`
- AC-3: `a_shard_with_a_writer_leaves_its_bytes_to_the_reclaim`, `a_deleted_writers_bytes_go_when_the_intent_falls_due`, `a_tombstone_delete_releases_heads_and_purges_without_an_intent`
- AC-4: `a_delete_of_no_such_incarnation_reads_once_and_writes_nothing`, `a_delete_that_loses_a_race_leaves_only_an_intent_the_reclaim_drops`
- AC-5: `pinned_shards_keep_their_bytes_and_unreadable_pins_purge_nothing`, `a_purge_pages_through_a_large_shard_and_counts_a_failure`, `a_reclaim_removes_only_the_dead_incarnation`
- AC-6: `a_drain_lists_nothing_past_the_first_bucket_not_yet_due`, `a_drain_moves_past_a_thousand_kept_intents`, `a_page_ending_in_another_nodes_bucket_skips_the_rest_of_it`, `an_intent_is_one_put_filed_under_the_minute_it_falls_due`, `an_intent_is_due_only_once_its_whole_minute_has_passed`, `a_drain_stops_at_the_first_bucket_not_yet_due`, `a_bucket_this_node_does_not_own_is_left_to_its_owner`, `a_failed_reclaim_keeps_its_intent_and_a_corrupt_one_is_dropped`, `a_failed_get_keeps_the_intent`, `a_drain_pages_through_a_thousand_due_intents_and_more`
- AC-7: `an_intent_is_written_byte_for_byte_as_the_java_concatenates_it`, `a_written_intent_reads_back_and_a_short_buffer_says_what_it_needs`, `had_writer_is_false_only_for_the_literal_false`, `unknown_fields_are_skipped_and_order_does_not_matter`, `an_intent_missing_or_mistyping_a_field_is_corrupt`, `a_repeated_field_is_corrupt_as_jackson_strict_detection_refuses_it`, `a_truncated_or_trailing_intent_is_corrupt`, `a_bucket_is_the_utc_minute_a_due_time_falls_in`, `a_bucket_starts_at_its_minute_and_round_trips`, `a_name_that_is_not_a_real_minute_has_no_start`, `an_intent_key_is_filed_under_its_bucket_by_name_and_uuid`
- AC-8: `invalid_arguments_fail_before_any_attempt`, `invalid_arguments_fail_before_any_request`
