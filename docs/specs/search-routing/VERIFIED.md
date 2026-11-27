# Verified: `routing` and `preference` on `_search` and `_msearch`

- AC-1: `a_routing_value_visits_the_shard_it_routes_to` in crates/node/tests/it/search_routing.rs (seen red before the parameter was read, green after)
- AC-2: `several_values_visit_each_shard_and_an_empty_one_visits_all` in crates/node/tests/it/search_routing.rs
- AC-3: `several_values_visit_each_shard_and_an_empty_one_visits_all` in crates/node/tests/it/search_routing.rs
- AC-4: `preference_is_accepted_and_changes_nothing` in crates/node/tests/it/search_routing.rs
- AC-5: `an_msearch_header_routes_and_prefers` in crates/node/tests/it/search_routing.rs
