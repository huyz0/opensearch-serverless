# Reader: explain: verified

Run with `cargo test -p oss-reader -p oss-compat` on 2026-11-17 (Linux cloud container); the new fixture steps recorded from stock OpenSearch 3.10.0-SNAPSHOT build `b9f09249`.

- AC-1: `the_reader_node_answers_every_search_fixture_as_opensearch_did`
- AC-2: `each_hit_carries_its_shard_node_and_explanation_after_its_sort`, `an_explanation_is_written_as_value_description_and_details`
- AC-3: `a_query_the_engine_does_not_explain_as_opensearch_does_is_refused`
- AC-4: `explanations_name_the_queries_as_opensearch_names_them`
