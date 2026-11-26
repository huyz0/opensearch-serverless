# Nested fields: verified

Run on 2026-11-26 (Linux cloud container, emulator only). Doc and writer tests were written first and seen failing (the writer's two shard tests read `[true, true]` where a block of five and two documents was expected before `index_routed` wrote blocks). The node tests were written after the plumbing and were not seen red; AC-10's refusal and the config test cover the switch both ways.

- AC-1: `nested_objects_parse_alone_inside_nested_and_inside_plain_objects`, `a_nested_object_is_written_as_opensearch_writes_it`, `a_caller_that_does_not_index_nested_is_told_it_is_unsupported`, `what_is_not_indexed_in_a_nested_object_is_unsupported` (crates/doc/tests/nested_mapping.rs)
- AC-2: `merging_a_nested_object_with_a_plain_one_fails_either_way`, `more_than_a_hundred_nested_objects_are_refused`
- AC-3: `an_array_of_nested_objects_is_one_child_an_element_and_the_parents_fields_leave_them_out`, `one_nested_object_is_one_child`, `a_nested_object_inside_another_is_written_before_it`, `no_object_no_array_and_null_give_no_child`, `a_concrete_value_where_a_nested_object_is_mapped_fails_as_for_an_object` (crates/doc/tests/nested_document.rs)
- AC-4: `more_than_ten_thousand_nested_objects_fail_with_the_javas_message`
- AC-5: `a_new_field_under_a_nested_object_grows_its_mapping_and_lands_in_the_child`
- AC-6: `a_block_is_the_children_in_order_and_then_the_document`, `only_the_document_has_its_metadata`, `a_document_without_nested_objects_is_the_one_document_it_was` (crates/doc/tests/nested_lucene.rs)
- AC-7: `a_document_is_a_block_an_update_replaces_it_and_a_delete_removes_it`, `a_document_that_fails_to_parse_writes_no_child`, `a_replayed_record_is_a_block_and_a_replayed_update_replaces_it` (crates/writer/tests/nested_shard.rs; the replay test was seen red against the old replay)
- AC-8: `an_index_with_a_nested_object_never_takes_the_fast_plan`
- AC-9: `a_nested_index_is_written_searched_updated_and_deleted_through_a_node` (crates/node/tests/nested.rs); `a_copy_is_the_root_and_the_block_is_hidden_with_it` (crates/reader/tests/overlay_commit.rs: the overlay finds only the root as the commit's copy and hides its block). NOT-RUN: a node test holding a nested document only in the log beside a published one; the pieces are tested apart.
- AC-10: `a_nested_mapping_is_refused_with_the_switch_off`, `an_index_made_with_the_switch_on_still_reads_and_writes_with_it_off`, `nested_is_off_by_default_and_turned_on_by_its_variable`

NOT-RUN: the 10,000-document soak the default flip waits for; comparison with recorded OpenSearch answers.
