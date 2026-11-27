# Verified: search on an alias of one index

- AC-1: `a_search_of_an_alias_of_one_index_answers_as_the_index_does` in crates/node/tests/it/search_alias.rs (seen red with the resolution switched off, green with it on)
- AC-2: `a_count_and_an_msearch_item_take_the_alias_too` in crates/node/tests/it/search_alias.rs
- AC-3: `an_alias_that_moved_or_was_removed_is_read_again` in crates/node/tests/it/search_alias.rs
- AC-4: replaced: the refusal it pinned is gone, and an alias of several is held by `an_alias_of_several_is_the_list_and_a_repeat_is_searched_once` in crates/node/tests/it/search_several.rs
- AC-5: `a_concrete_index_never_asks_the_source_and_adds_no_request` in crates/node/tests/it/search_alias.rs: the alias search adds the one GET of its own name's failed open, counted exactly; the source's register read is the registry's own, held by `the_registry_reads_an_alias_and_knows_an_index_is_none` in crates/node/src/search_alias.rs
- AC-6: `a_name_that_is_neither_is_not_found_and_no_source_changes_nothing` in crates/node/tests/it/search_alias.rs
- AC-7: `field_caps_of_an_alias_names_the_index_it_stands_for` in crates/node/tests/it/search_alias.rs (seen red with the step left out, green with it)
