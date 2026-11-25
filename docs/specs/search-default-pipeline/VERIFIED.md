# Index default search pipeline: verified

Run on 2026-11-25 in a Linux cloud sandbox (emulator store, so numbers are provisional): `cargo test -p oss-node --test it -- search_pipeline`, `cargo test -p oss-node --all-features --test server -- search_default_pipeline_setting`.

- AC-1: `a_create_takes_the_key_when_search_pipelines_are_on`, `a_create_refuses_the_key_when_search_pipelines_are_off`
- AC-2: `an_update_sets_replaces_and_removes_the_key_when_on`, `an_update_refuses_the_key_when_off`
- AC-3: `an_index_default_runs_when_the_search_names_none`
- AC-4: `an_index_default_runs_when_the_search_names_none`
- AC-5: `an_index_default_runs_when_the_search_names_none`
- AC-6: `an_index_with_no_default_makes_no_request_with_the_source_installed`
- AC-7: `an_index_default_runs_when_the_search_names_none`
- AC-8: `a_default_is_ignored_when_search_pipelines_are_off`
- AC-9: `a_default_never_precedes_the_point_in_time_check`
