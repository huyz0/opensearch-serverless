# Forwarded aggregations: single-value metrics: verified

Run on 2026-11-27 (Linux cloud container). The expected bytes are read from the 3.10.0 Java source; no Java-written fixture backs them (said in the spec). The tests were written beside the code and passed at their first run except `the_aggregations_of_a_request_reach_the_source`, which failed first because the body's JSON map sorted the aggregations, and passed once they are written in request order.

- AC-1: `a_written_request_reads_back` (`oss-transport`, `search_shard/aggs.rs`)
- AC-2: `another_type_and_a_sub_aggregation_are_refused`, `the_first_unsupported_item_is_named_and_nothing_after_it_is_read` (`crates/transport/tests/search_shard.rs`)
- AC-3: `a_reply_is_a_list_of_named_writeables`
- AC-4: `aggregations_asked_for_come_back_as_internal_aggregations`, `the_aggregations_of_a_request_reach_the_source`
- AC-5: `a_date_field_is_answered_with_the_date_time_format`, `a_date_aggregation_carries_the_date_format_and_another_kind_is_refused`
- AC-6: `a_stats_aggregation_is_answered_with_its_count_min_max_and_sum` (`oss-node`), and the `stats` request and answer in the recordings below
- AC-7: `the_metric_builders_are_read_and_written_as_java_wrote_them`, `the_metric_answers_are_the_bytes_java_wrote`, `a_date_answer_is_the_bytes_java_wrote` (`crates/transport/tests/search_shard.rs`)

Recorded on 2026-11-27 against the 3.10.0 jars from Maven Central: `AggRecordings.java` wrote `java-aggs-3.10.0.txt`, and the bytes written for slices 1 and 2 from the source matched them at the first run, so no expectation changed. The recorded shard results carry null metadata, as a shard's do; the `stats` code was written before its test and the test was not seen to fail first.
