# Cluster routes: verified

Run on 2026-11-20 in a Linux sandbox, emulator numbers provisional: `cargo test -p oss-node --features reader,writer --test cluster_routes --test cat_routes --test health_store --test cluster_assemble`.

- AC-1: `a_settings_read_nests_or_flattens_the_registers_persistent_settings`.
- AC-2: `an_update_answers_what_the_register_holds_and_merges_and_removes`.
- AC-3: `a_refused_update_answers_the_coordinators_words_and_writes_nothing`.
- AC-4: `an_unscoped_health_answers_the_node_half_and_says_it_is_partial` and `a_scoped_health_counts_each_indexs_shards_and_their_states`.
- AC-5: `a_missing_index_is_404_and_what_cannot_be_read_is_red_not_an_error`, and `the_store_source_reads_the_descriptor_and_a_head_a_shard` for the store source's reads.
- AC-6: `wait_for_nodes_waits_and_a_wait_that_ran_out_is_408`.
- AC-7: `the_parameters_the_shell_refuses_are_501_and_its_hints_are_accepted`.
- AC-8: `cat_health_is_one_row_in_text_or_json`.
- AC-9: `cat_indices_answers_names_lists_and_prefixes_and_refuses_paging` and `a_prefix_past_the_pattern_cap_is_refused_rather_than_cut`.
- AC-10: `list_indices_pages_in_name_order_and_the_token_resumes` and `list_indices_refuses_what_plan_refuses`.
- AC-11: `list_indices_refuses_what_plan_refuses` (the list route), `cat_health_is_one_row_in_text_or_json` (`_cat/health`) and `cat_indices_answers_names_lists_and_prefixes_and_refuses_paging`.
- AC-12: `requests_are_what_the_spec_says` and `a_lapsed_claim_by_an_owner_with_no_lease_is_dormant_and_asks_once_an_owner`.
- Assembly: `the_settings_and_cat_routes_are_always_served_and_health_once_a_source_is_given`.
