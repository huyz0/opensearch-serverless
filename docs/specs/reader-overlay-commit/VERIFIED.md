# Verified: the overlay finds the commit's copies of the ids it replaces

- AC-1: `copies_are_found_across_segments_with_their_sequence_numbers` in crates/reader/tests/overlay_commit.rs
- AC-2: `candidates_are_the_ids_above_the_checkpoint_once_in_first_seen_order` in crates/reader/tests/overlay_commit.rs
- AC-3: `a_record_the_commit_already_holds_is_dropped` in crates/reader/tests/overlay_commit.rs
- AC-4: `hidden_lists_the_commit_copies_of_replaced_and_deleted_ids` in crates/reader/tests/overlay_commit.rs
- AC-5: `no_ids_find_nothing` in crates/reader/tests/overlay_commit.rs
- AC-6: `the_lookup_takes_at_most_the_most_ids` in crates/reader/tests/overlay_commit.rs
