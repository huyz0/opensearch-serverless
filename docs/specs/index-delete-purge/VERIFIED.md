# Verified

- AC-1: `a_shards_blobs_are_deleted_in_one_request`, and `a_purge_pages_through_a_large_shard_and_counts_a_failure` (3 DELETEs for 1,001 blobs and the descriptor, where it was 1,002). Emulator, provisional.
- AC-2: `an_index_above_the_inline_limit_is_purged_at_the_reclaim`
- AC-3: `an_index_within_the_inline_limit_is_purged_inline`
- AC-4: `a_pinned_shard_is_purged_once_its_pin_ends`
- AC-5: `a_pinned_shard_is_left_as_the_java_does_by_default`
- AC-6: `a_large_index_is_purged_at_the_reclaim_and_a_pinned_shard_when_its_view_ends`
