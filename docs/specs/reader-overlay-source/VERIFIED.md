# Verified: the node searches over an installed overlay source

- AC-1: `with_the_setting_off_no_source_is_installed_and_the_commit_answers` in crates/node/tests/overlay_search.rs
- AC-2: `an_installed_source_s_overlay_replaces_deletes_and_adds_what_a_search_sees` in crates/node/tests/overlay_search.rs
- AC-3: `a_source_with_nothing_finished_leaves_the_commits_answer` in crates/node/tests/overlay_search.rs
- AC-4: `searches_between_two_overlays_share_one_view_and_make_no_request` and `a_new_overlay_from_the_source_makes_a_new_view_once` in crates/node/tests/overlay_search.rs
- AC-5: `the_setting_is_off_by_default` in crates/node/tests/overlay_search.rs, and `the_nodes_settings_form_a_valid_registry` in crates/node/src/settings.rs
