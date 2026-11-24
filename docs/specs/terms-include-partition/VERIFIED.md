# Verified: a partition-based `include` of a keyword `terms`

- AC-1: `a_partition_include_is_parsed_and_checked_as_opensearch_checks_it` in crates/reader/src/aggs/bucket/keyword_options_tests.rs
- AC-1: `a_partition_include_keeps_the_terms_whose_murmur_hash_falls_in_it` in crates/reader/src/aggs/bucket/keyword_partition_tests.rs
