# Verified: `_render/template` and `_search/template`

- AC-1: `a_template_renders_over_its_params` in crates/node/tests/it/search_template.rs, and the 141 recorded cases of `fixtures/mustache/render.json` replayed by `every_recorded_case_renders_as_the_java_does` in crates/reader/src/template/tests.rs
- AC-2: `a_search_template_hits_what_the_same_search_hits` and `a_two_shard_index_searches_through_the_coordinator` in crates/node/tests/it/search_template.rs
- AC-3: `the_json_join_and_url_functions_render_as_java_does` in crates/node/tests/it/search_template.rs
- AC-4: `a_template_that_does_not_compile_is_a_script_exception` in crates/node/tests/it/search_template.rs
- AC-5: `a_request_that_is_no_template_is_refused_as_stock_refuses_it` in crates/node/tests/it/search_template.rs
- AC-6: `what_this_node_lacks_is_refused` in crates/node/tests/it/search_template.rs, and `a_template_that_nests_too_deep_or_renders_too_much_is_refused` in crates/reader/src/template/tests.rs
- AC-7: `rendered_text_that_is_no_search_body_is_a_search_error` in crates/node/tests/it/search_template.rs
- AC-8: `a_warm_index_adds_no_store_request` in crates/node/tests/it/search_template.rs
