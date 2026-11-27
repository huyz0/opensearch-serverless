# Verified: auto_date_histogram aggregation

- AC-1: `a_body_is_read_as_java_reads_it` in crates/reader/src/aggs/bucket/auto_date_tests.rs
- AC-2: `the_final_reduce_gives_the_bytes_java_wrote` in crates/reader/src/aggs/bucket/auto_date_tests.rs
- AC-3: `a_shard_counts_the_documents_in_the_rounding_its_data_fits` in crates/reader/src/aggs/bucket/auto_date_tests.rs
- AC-4: `typed_keys_name_the_result_by_its_type` in crates/reader/src/aggs/bucket/auto_date_tests.rs
- AC-5: `an_auto_date_histogram_picks_its_interval` in crates/node/tests/it/search_auto_date.rs
- AC-6: `a_sub_aggregation_is_refused` in crates/reader/src/aggs/bucket/auto_date_tests.rs

The recordings (`fixtures/auto-date-histogram`) are the 3.10.0 builder's parse of 33 bodies and `InternalAutoDateHistogram.reduce` over 13 hand-built shard results. The reduce was written from the bytecode and the recordings agreed at the first run. The shard side is not recorded: the data in the shard test sits in one rounding, so rounding up on a shard is checked only through the reduce. The full gate, coverage and mutation are left to CI.
