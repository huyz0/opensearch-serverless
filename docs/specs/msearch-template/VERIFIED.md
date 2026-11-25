# Verified: `_msearch/template`

- AC-1: `each_item_is_its_searchs_response_with_a_status` and `a_two_shard_index_searches_through_the_coordinator` in crates/node/tests/it/msearch_template.rs (seen red with the body line parsed as a search body)
- AC-2: `a_bad_item_fails_alone_in_its_slot` in crates/node/tests/it/msearch_template.rs (a template that does not compile)
- AC-3: `a_bad_item_fails_alone_in_its_slot` in crates/node/tests/it/msearch_template.rs (no template, a stored id, a profile)
- AC-4: `the_paths_index_is_the_default` in crates/node/tests/it/msearch_template.rs
- AC-5: `an_empty_body_and_an_unterminated_one_are_the_errors_msearch_gives` in crates/node/tests/it/msearch_template.rs
- AC-6: `a_warm_index_adds_no_store_request` in crates/node/tests/it/msearch_template.rs
- AC-7: `routes_whose_indices_are_in_the_body_or_that_are_proxied_need_all_access` in crates/node/tests/server/security_authz.rs
