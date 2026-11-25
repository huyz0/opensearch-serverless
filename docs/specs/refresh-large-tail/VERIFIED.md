# Verified: refresh-large-tail

- AC-1: `ascii_and_mixed_strings_round_trip_on_the_same_bytes` in crates/transport/tests/wire/stream.rs
- AC-2: `the_first_count_after_a_refresh_sees_every_acknowledged_large_document` in crates/node/tests/refresh_large_tail.rs
- AC-3: `the_first_count_after_a_refresh_sees_every_large_document_with_a_pipeline` in crates/node/tests/refresh_large_tail.rs (failed with a 404, then a count of 0 before the owner was named as the empty commit's writer; passes three runs in a row), and in crates/reader/tests/it/never_written.rs `a_shard_owned_and_not_yet_published_opens_empty_with_its_owner_as_writer`, `an_owner_whose_lease_ran_out_or_an_earlier_index_or_no_head_is_not_found`, `a_bucketed_index_is_not_found_when_asked_for_an_unpublished_commit`.
