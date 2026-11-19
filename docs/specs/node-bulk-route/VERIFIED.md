# Verified: a writer node serves `_bulk`

- AC-1: `a_bulk_applies_its_items_by_post_and_put_with_and_without_an_index` in crates/node/tests/bulk_route.rs
- AC-2: `a_refused_item_fails_alone` in crates/node/tests/bulk_route.rs
- AC-3: `refresh_follows_the_parameter` in crates/node/tests/bulk_route.rs
- AC-4: `a_bad_body_and_an_unknown_index_are_answered` in crates/node/tests/bulk_route.rs
- AC-5: `a_body_of_several_slices_is_applied_whole` in crates/node/tests/bulk_route.rs
- AC-6: `an_item_without_an_id_gets_a_time_based_one` and `require_alias_refuses_an_index_that_is_not_one` in crates/node/tests/bulk_route.rs
