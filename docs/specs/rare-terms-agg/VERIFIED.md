# Rare terms aggregation: verified

Run on 2026-11-27 (Linux cloud container). The expected values are read from the 3.10.0 Java (the two `max_doc_count` messages and the `precision` message from the jar's constants); no stock recording backs them (said in the spec). The tests were written after the code and passed at their first run, so none was seen red first.

- AC-1: `the_body_parses_with_its_defaults_and_options`, `bad_and_unserved_options_fail_as_java_fails` (`oss-reader`)
- AC-2: `a_shard_keeps_the_terms_of_few_documents_and_excludes_the_others`
- AC-3: `a_reduce_adds_counts_and_drops_what_any_shard_excluded`, `a_reduce_drops_a_term_whose_sum_passes_the_maximum`
- AC-4: `the_response_lists_each_bucket_by_key_and_count`, `an_unmapped_field_answers_nothing`
- AC-5: `sub_aggregations_and_nesting_are_refused`; the `too_many_buckets_exception` of a field past the bucket limit is NOT-RUN: no test index holds 65,536 terms
- AC-6: `a_long_field_is_answered_with_numbers_as_lrareterms`; the refusal of a date field is NOT-RUN: no test index maps a date field
