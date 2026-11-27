# Verified: the coordinator decides what an update does to the document it read

- AC-1: `options_are_refused_in_the_shells_order` in crates/coordinator/tests/it/update.rs
- AC-2: `attempts_are_one_plus_the_retries` in crates/coordinator/tests/it/update.rs
- AC-3: `a_changed_document_conflicts_with_a_set_condition` in crates/coordinator/tests/it/update.rs
- AC-3 (the unset number): `the_unset_condition_is_the_shells_number` in crates/coordinator/tests/it/update.rs
- AC-4: `a_missing_document_is_created_from_the_doc_or_upsert_or_refused` in crates/coordinator/tests/it/update.rs
- AC-5: `a_found_document_is_scripted_or_merged` in crates/coordinator/tests/it/update.rs
- AC-6: `a_scripts_operation_is_kept_only_where_it_makes_sense` in crates/coordinator/tests/it/update.rs
- AC-7: `the_write_carries_the_callers_condition_or_the_one_just_read` in crates/coordinator/tests/it/update.rs
- AC-8: `an_update_script_edits_the_source_and_chooses_the_operation` in crates/coordinator/tests/update_script.rs (run 2026-11-27: `cargo test -p oss-coordinator --test update_script`)
