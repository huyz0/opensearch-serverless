# Verified: bucket_script and bucket_selector

- AC-1: `both_pipelines_give_the_bytes_java_wrote_for_each_gap_policy` in crates/reader/src/aggs/pipeline/script_tests.rs
- AC-2: `a_script_result_is_written_as_java_writes_it` in crates/reader/src/aggs/pipeline/script_tests.rs
- AC-3: `a_typed_result_is_a_simple_value` in crates/reader/src/aggs/pipeline/script_tests.rs
- AC-4: `a_script_that_fails_or_returns_the_wrong_type_fails_the_search` in crates/reader/src/aggs/pipeline/script_tests.rs
- AC-5: `a_body_is_read_as_java_reads_it` in crates/reader/src/aggs/pipeline/script_tests.rs
- AC-6: `a_parsed_pipeline_keeps_its_script_variables_and_host` in crates/reader/src/aggs/pipeline/script_tests.rs
- AC-7: `the_switch_gates_both_and_the_top_level_refuses_them` in crates/reader/src/aggs/pipeline/script_tests.rs
- AC-8: `the_scripts_of_two_pipelines_run_over_the_buckets` in crates/node/tests/script_pipelines.rs

The recordings (`fixtures/pipeline-script`) are the 3.10.0 builders' parse of 23 bodies for each type and `reduce` over the recorder's four-bucket histogram, run through a test script engine whose scripts are named by their source; the Painless equivalents in the tests are this node's. The Painless runs themselves are not recorded against Java's Painless. The full gate, coverage and mutation are left to CI.
