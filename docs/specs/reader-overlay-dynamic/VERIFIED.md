# Verified: reader-overlay-dynamic

- AC-1: `an_empty_mapping_source_builds_over_records_whose_fields_are_all_new` in crates/reader/tests/it/overlay_dynamic.rs
- AC-2: `a_later_record_uses_a_field_an_earlier_one_added` in crates/reader/tests/it/overlay_dynamic.rs
- AC-3: `a_dynamic_field_whose_records_disagree_on_type_names_the_record` in crates/reader/tests/it/overlay_dynamic.rs
- AC-4: `an_index_without_a_mapping_is_served_by_the_overlay_at_the_cost_of_one_with` in crates/node/tests/overlay_dynamic.rs
- AC-5: `an_index_without_a_mapping_is_served_by_the_overlay_at_the_cost_of_one_with` in crates/node/tests/overlay_dynamic.rs
- AC-6: `an_index_without_a_mapping_is_served_by_the_overlay_at_the_cost_of_one_with` in crates/node/tests/overlay_dynamic.rs
- AC-7: `an_index_without_a_mapping_is_served_by_the_overlay_at_the_cost_of_one_with` in crates/node/tests/overlay_dynamic.rs
- AC-8: `an_index_without_a_mapping_is_served_by_the_overlay_at_the_cost_of_one_with` in crates/node/tests/overlay_dynamic.rs (searches nodes 0, 1 and 2; the owner is one of them), 0 of 6 runs green before the change and 3 of 3 after

Provisional (emulator, divisor 4): 20 writes cost 96 requests on the mapped index and 100 on the unmapped one, both served on two other nodes within 3 s.
