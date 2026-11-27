# Verified: `show_term_doc_count_error` of a terms aggregation

- AC-1: `a_bucket_writes_the_error_of_the_shards_that_lack_it` in crates/reader/src/aggs/bucket/terms_tests.rs
- AC-2: `a_bucket_writes_the_error_of_the_shards_that_lack_it` in crates/reader/src/aggs/bucket/terms_tests.rs
- AC-3: `an_unbounded_shard_makes_every_bucket_error_unbounded` in crates/reader/src/aggs/bucket/terms_tests.rs
- AC-4: `a_keyword_bucket_writes_the_error_of_the_shards_that_lack_it` in crates/reader/src/aggs/bucket/keyword_terms_tests.rs
- AC-5: `a_bucket_carries_the_error_of_the_shards_that_lack_it` in crates/coordinator/tests/it/terms_reduce.rs
- AC-6: `show_term_doc_count_error_parses_as_a_boolean` in crates/reader/src/aggs/bucket/terms_tests.rs
- AC-7: `a_running_sum_that_reaches_minus_one_stops_adding_as_java_does` in crates/reader/src/aggs/bucket/terms_error_tests.rs (cases recorded by `fixtures/terms-error/ErrorRecordings.java`)
