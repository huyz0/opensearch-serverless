# Verified: node-frozen-search-shards

- AC-1: `a_search_of_a_view_of_two_shards_answers_what_a_live_search_of_them_does` in crates/node/tests/it/frozen_search.rs
- AC-2: `a_view_of_two_shards_does_not_read_the_live_commits` in crates/node/tests/it/frozen_search.rs
- AC-3: `a_second_search_of_a_view_of_two_shards_reads_no_index_and_holds_an_entry_each` in crates/node/tests/it/frozen_search.rs
