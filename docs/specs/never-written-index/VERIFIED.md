# Verified: a read of an index that never held a document

- AC-1: `a_search_answers_200_with_no_hits` in crates/node/tests/it/never_written.rs
- AC-2: `a_count_answers_zero` in crates/node/tests/it/never_written.rs
- AC-2: `a_search_with_an_aggregation_answers_empty_buckets` in crates/node/tests/it/never_written.rs
- AC-3: `a_search_of_an_index_that_does_not_exist_is_still_404` in crates/node/tests/it/never_written.rs
- AC-4: `the_reads_of_an_unwritten_index_make_no_list_and_no_write` in crates/node/tests/it/never_written.rs
- AC-4: `an_index_with_a_descriptor_and_no_commit_opens_with_no_documents` in crates/reader/tests/it/never_written.rs
- AC-5: `a_bucketed_index_with_no_bucket_manifest_is_an_error` in crates/reader/tests/it/never_written.rs
- AC-6: `a_shard_a_writer_took_that_lost_its_commit_register_is_an_error_not_an_empty_shard` in crates/reader/tests/it/never_written.rs
- AC-6: `a_head_an_earlier_index_of_the_name_left_does_not_make_the_new_one_lose_a_commit` in crates/reader/tests/it/never_written.rs
- AC-5: `a_bucketed_member_with_no_commit_yet_opens_with_no_documents` in crates/reader/tests/it/never_written.rs
- AC-5: `a_member_without_a_commit_is_empty_and_one_without_a_manifest_entry_is_not_followable` in crates/reader/tests/it/open_bucket.rs
