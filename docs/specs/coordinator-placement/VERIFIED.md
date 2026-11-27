# Verified: the coordinator places a document and decides what a failed forward means

- AC-1: `a_system_index_is_refused_before_it_is_read_and_a_missing_one_is_not_found` in crates/coordinator/tests/it/place.rs
- AC-2: `a_placement_has_the_shard_the_uuid_the_local_flag_and_the_owner_read_once` in crates/coordinator/tests/it/place.rs
- AC-3: `a_failure_forgets_the_hint_and_the_memo_first` in crates/coordinator/tests/it/place.rs
- AC-4: `a_lost_condition_and_an_owners_client_answer_are_rethrown_and_a_server_one_is_not_here` in crates/coordinator/tests/it/place.rs
- AC-5: `a_deadline_or_a_dropped_connection_is_never_retried_and_doubts_ownership` in crates/coordinator/tests/it/place.rs
- AC-6: `a_non_owner_or_unreachable_owner_reads_the_head_once_and_forwards_or_refuses` in crates/coordinator/tests/it/place.rs and `an_owner_with_no_lease_keeps_its_own_words_and_no_retry_reads_no_head` in crates/coordinator/tests/it/place.rs
- AC-7: `not_here_words_no_owner_this_node_and_another_node` in crates/coordinator/tests/it/place.rs
