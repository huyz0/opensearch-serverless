# Verified: `shard_min_doc_count` of a terms aggregation

- AC-1: `shard_min_doc_count_is_capped_at_min_doc_count_and_defaults_to_zero` in crates/reader/src/aggs/bucket/terms_tests.rs
- AC-1: `a_negative_shard_min_doc_count_fails_as_opensearch_fails_it` in crates/reader/src/aggs/bucket/terms_tests.rs
- AC-2: `a_shard_min_doc_count_that_bites_needs_the_count_descending_order` in crates/reader/src/aggs/bucket/terms_tests.rs
- AC-3: `a_shard_min_doc_count_drops_the_shards_thin_buckets_into_other` in crates/reader/src/aggs/bucket/collect_tests.rs
- AC-3: `a_keyword_shard_min_doc_count_drops_the_twelves_into_other` in crates/reader/src/aggs/bucket/keyword_collect_tests.rs
- AC-3: `a_shard_drops_buckets_under_its_minimum_and_counts_their_documents_as_others` in crates/reader/src/aggs/bucket/terms_tests.rs
- AC-3: `a_keyword_shard_drops_buckets_under_its_minimum_with_their_sub_lists` in crates/reader/src/aggs/bucket/keyword_terms_tests.rs
