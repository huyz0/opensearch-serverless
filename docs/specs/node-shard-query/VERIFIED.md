# Verified: a node runs one shard's query for a coordinator in the same process

- AC-1: `a_shard_query_finds_what_the_http_search_finds_and_writes_its_hits_the_same` in crates/node/tests/it/shards.rs
- AC-2: `a_shard_past_the_first_is_queried_and_its_hits_name_it` in crates/node/tests/it/shards.rs
- AC-3: `a_failed_shard_query_names_the_shard` in crates/node/tests/it/shards.rs
