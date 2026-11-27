# Forwarded aggregations: single-value metrics: verified

Run on 2026-11-27 (Linux cloud container). The expected bytes are read from the 3.10.0 Java source; no Java-written fixture backs them (said in the spec). The tests were written beside the code and passed at their first run except `the_aggregations_of_a_request_reach_the_source`, which failed first because the body's JSON map sorted the aggregations, and passed once they are written in request order.

- AC-1: `a_written_request_reads_back` (`oss-transport`, `search_shard/aggs.rs`)
- AC-2: `another_type_and_a_sub_aggregation_are_refused`, `the_first_unsupported_item_is_named_and_nothing_after_it_is_read` (`crates/transport/tests/search_shard.rs`)
- AC-3: `a_reply_is_a_list_of_named_writeables`
- AC-4: `aggregations_asked_for_come_back_as_internal_aggregations`, `the_aggregations_of_a_request_reach_the_source`
- AC-5: `a_date_field_is_answered_with_the_date_time_format`, `a_date_aggregation_carries_the_date_format_and_another_kind_is_refused`
- AC-6: `a_stats_aggregation_is_answered_with_its_count_min_max_and_sum` (`oss-node`), and the stats request and answer in the recordings below
- AC-7: `the_metric_builders_are_read_and_written_as_java_wrote_them`, `the_metric_answers_are_the_bytes_java_wrote`, `a_date_answer_is_the_bytes_java_wrote` (`crates/transport/tests/search_shard_aggs.rs`)
- AC-8: `the_cardinality_builders_are_the_bytes_java_wrote`, `a_cardinality_answer_is_the_bytes_java_wrote_around_the_sketch`, `an_execution_hint_is_refused` (`crates/transport/tests/search_shard_aggs.rs`), `a_sketch_writes_the_bytes_java_wrote` (`oss-reader`), `a_cardinality_aggregation_is_answered_with_its_sketch`, `a_precision_threshold_reaches_the_source` (`oss-node`)

- AC-9: `the_percentiles_builders_are_the_bytes_java_wrote`, `a_percentiles_answer_is_the_bytes_java_wrote_around_the_digest`, `a_percentiles_method_other_than_tdigest_is_refused` (`crates/transport/tests/search_shard_aggs.rs`), `a_digest_writes_the_bytes_java_wrote` (`oss-reader`), `percentile_keys_and_options_reach_the_source`, `a_percentiles_aggregation_is_answered_with_its_digest_and_an_empty_one_with_the_compression` (`oss-node`)
- AC-10: `the_terms_builders_are_the_bytes_java_wrote`, `a_terms_with_a_sub_aggregation_is_refused_by_name`, `a_string_terms_answer_is_the_bytes_java_wrote`, `a_long_terms_answer_is_the_bytes_java_wrote` (`crates/transport/tests/search_shard_terms.rs`), `a_terms_aggregations_options_reach_the_source`, `a_keyword_terms_aggregation_is_answered_as_string_terms_in_the_requests_order`, `a_numeric_terms_aggregation_is_answered_as_long_terms_and_one_with_sub_aggregations_is_refused` (`oss-node`)

Recorded on 2026-11-27 against the 3.10.0 jars from Maven Central: `AggRecordings.java` wrote `java-aggs-3.10.0.txt`, and the bytes written for slices 1 and 2 from the source matched them at the first run, so no expectation changed. The recorded shard results carry null metadata, as a shard's do; the `stats` code was written before its test and the test was not seen to fail first.

Cardinality slice: `CardRecordings.java` wrote `java-cardinality-3.10.0.txt` (builders with and without `precision_threshold`; answers for no sketch, four hashes at precision 14, and 100 hashes at precision 4). Linear-counting hashes are written in table-slot order, which `Hll::to_bytes` matches; the tests were written after the code and not seen red first.

Percentiles slice: `PctRecordings.java` wrote `java-percentiles-3.10.0.txt` (builders with the defaults, with keys and a compression, and for ranks; answers over ten values, five hundred, none, and ranks). The Rust digest of the values 1 to 10 and an empty one write the same bytes as Java's; the 500-value digest is recorded only for the wire around it, since Java's random values are not reproduced here. The tests were written after the code and not seen red first.

Terms slice: `TermsRecordings.java` wrote `java-terms-3.10.0.txt` (builders with the defaults and with options; answers as `StringTerms` and `LongTerms`, with and without a sub-aggregation, and with no buckets). The recordings with sub-aggregations are for the next slice. A shard-level `shard_min_doc_count` with an order other than count descending is refused by the reader's parser, so that request fails as a bad request. The tests were written after the code and not seen red first.
