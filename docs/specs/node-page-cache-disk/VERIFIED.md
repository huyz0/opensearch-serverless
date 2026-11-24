# Verified: node page cache disk tier

- AC-1: `the_disk_tier_is_off_unless_asked_for_and_asked_for_with_a_size` in crates/node/tests/server/boot_page_cache.rs
- AC-2: `a_node_with_a_disk_tier_keeps_the_pages_it_read_and_answers_the_same` in crates/node/tests/server/boot_page_cache.rs
- AC-3: `the_tiers_records_are_charged_to_open_indexes_and_refused_when_they_do_not_fit` in crates/node/src/page_cache_disk.rs
