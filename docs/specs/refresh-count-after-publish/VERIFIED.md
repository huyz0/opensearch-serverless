# Verified: refresh-count-after-publish

- AC-1: `a_count_after_a_refresh_is_answered_when_a_publish_landed_since_the_shard_opened` in crates/node/tests/refresh_large_tail.rs
- AC-2: `a_count_after_a_refresh_is_answered_when_the_unpublished_tail_outgrew_the_keep` in crates/node/tests/refresh_large_tail.rs
- AC-3: `a_search_and_the_worker_never_build_the_same_overlay_twice` in crates/node/tests/overlay_builder.rs
