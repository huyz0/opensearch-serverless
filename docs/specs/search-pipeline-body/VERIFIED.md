# Search pipeline in the body and on `_msearch`: verified

Run on 2026-11-25 in a Linux cloud sandbox (emulator store, so numbers are provisional): `cargo test -p oss-reader --lib`, `cargo test -p oss-node --all-features --test it`.

- AC-1: `a_body_names_a_pipeline_and_the_parameter_overrides_it`, `a_body_names_a_search_pipeline_by_string_only`
- AC-2: `a_body_names_a_pipeline_and_the_parameter_overrides_it`, `a_body_names_a_search_pipeline_by_string_only`
- AC-3: `each_msearch_item_runs_its_own_pipeline_and_a_missing_one_fails_alone`
- AC-4: `a_body_pipeline_is_501_when_off_and_none_is_not`
- AC-5: `count_still_refuses_the_pipeline_parameter`
- AC-6: `an_unknown_parameter_is_refused_and_a_warm_run_makes_no_request`, `an_msearch_pays_one_marker_get_for_each_item_that_names_a_pipeline`
- AC-7: `a_caller_that_runs_no_pipeline_refuses_one`
