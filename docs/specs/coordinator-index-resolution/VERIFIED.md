# Verified: the coordinator resolves the names in a request

- AC-1: `a_list_resolves_in_request_order_once_each_and_an_alias_expands` in crates/coordinator/tests/it/resolve.rs
- AC-2: `an_empty_name_or_a_pattern_that_is_not_a_prefix_is_refused` in crates/coordinator/tests/it/resolve.rs
- AC-3: `a_prefix_pattern_resolves_up_to_the_cap_and_ghosts_are_not_matches` in crates/coordinator/tests/it/resolve.rs
- AC-4: `a_missing_name_is_refused_or_skipped_and_nothing_resolving_is_refused` in crates/coordinator/tests/it/resolve.rs
- AC-5: `an_alias_naming_a_gone_index_is_refused_or_skips_that_index` in crates/coordinator/tests/it/resolve.rs
- AC-6: `a_system_index_is_refused_when_typed_and_hidden_when_a_pattern_matched_it` in crates/coordinator/tests/it/resolve.rs
