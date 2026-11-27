# Forwarded aggregations: single-value metrics: verified

Run on 2026-11-27 (Linux cloud container). The expected bytes are read from the 3.10.0 Java source; no Java-written fixture backs them (said in the spec). The tests were written beside the code and passed at their first run except `the_aggregations_of_a_request_reach_the_source`, which failed first because the body's JSON map sorted the aggregations, and passed once they are written in request order.

- AC-1: `a_written_request_reads_back` (`oss-transport`, `search_shard/aggs.rs`)
- AC-2: `another_type_and_a_sub_aggregation_are_refused`, `the_first_unsupported_item_is_named_and_nothing_after_it_is_read` (`crates/transport/tests/search_shard.rs`)
- AC-3: `a_reply_is_a_list_of_named_writeables`
- AC-4: `aggregations_asked_for_come_back_as_internal_aggregations`, `an_aggregation_over_a_date_or_of_another_kind_is_refused`, `the_aggregations_of_a_request_reach_the_source`
