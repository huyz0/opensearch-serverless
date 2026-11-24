# Verified: the writer answers a reader's tail fetch

- AC-1: `a_shard_not_written_here_or_in_doubt_answers_only_the_unknown_claim` in crates/node/tests/tail/tail_serve.rs
- AC-2: `off_a_check_and_a_gap_answer_the_claim_and_subscribe_nobody` in crates/node/tests/tail/tail_serve.rs
- AC-3: `a_new_fetch_is_subscribed_before_it_reads_and_ids_count_from_zero` in crates/node/tests/tail/tail_serve.rs
- AC-4: `naming_the_held_subscription_renews_it_and_anything_else_subscribes_anew` in crates/node/tests/tail/tail_serve.rs
- AC-5: `the_claim_is_the_highest_the_first_time_is_raised_and_never_lowered` in crates/node/tests/tail/tail_serve.rs
- AC-6: `unsubscribe_removes_one_node_and_released_removes_all_and_the_keep` in crates/node/tests/tail/tail_serve.rs
- AC-7: `a_grant_is_six_hundred_milliseconds_from_now` in crates/node/tests/tail/tail_serve.rs
- AC-3 (whole with its records): `a_snapshot_reads_the_records_and_whether_they_are_whole_together` in crates/node/tests/tail/tail_keep.rs
- AC-5 (a keep begun by a fetch): `a_keep_begun_by_a_fetch_raises_a_lower_claim_to_its_floor` in crates/node/tests/tail/tail_serve.rs
