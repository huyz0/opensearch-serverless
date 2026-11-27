# Verified: moving_fn

- AC-1: `the_window_is_what_java_gives_each_function` in crates/reader/src/aggs/pipeline/moving_tests.rs
- AC-2: `a_path_that_names_nothing_leaves_the_buckets_as_they_were` in crates/reader/src/aggs/pipeline/moving_tests.rs
- AC-3: `a_typed_result_is_a_simple_value` in crates/reader/src/aggs/pipeline/moving_tests.rs
- AC-4: `a_script_that_throws_fails_the_search` in crates/reader/src/aggs/pipeline/moving_tests.rs
- AC-5: `a_body_is_read_as_java_reads_it` in crates/reader/src/aggs/pipeline/moving_tests.rs
- AC-6: `a_parsed_moving_fn_keeps_its_window_shift_and_host` in crates/reader/src/aggs/pipeline/moving_tests.rs
- AC-7: `the_switch_gates_it_and_only_a_histogram_holds_it` in crates/reader/src/aggs/pipeline/moving_tests.rs
- AC-8: `a_moving_fn_runs_over_the_buckets` in crates/node/tests/script_pipelines.rs

The recordings (`fixtures/pipeline-script/java-moving-3.10.0.txt`) are the 3.10.0 builder's parse of 25 bodies and `MovFnPipelineAggregator.reduce` over the recorder's four-bucket histogram, run through a test script engine whose scripts call `MovingFunctions` as a Painless script does. The top-level and non-histogram validation messages are read from the 3.10.0 bytecode constants, not recorded. The full gate, coverage and mutation are left to CI.
