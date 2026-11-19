# Verified: the coordinator decides the index admin calls that read and delete

- AC-1: `reserved_and_backing_names_are_refused` in crates/coordinator/tests/index_admin.rs
- AC-2: `the_view_follows_the_path_and_the_setting_name` in crates/coordinator/tests/index_admin.rs
- AC-3: `a_stream_write_index_cannot_be_deleted` in crates/coordinator/tests/index_admin.rs
- AC-4: `a_failed_delete_is_a_conflict_or_not_found` in crates/coordinator/tests/index_admin.rs
- AC-5: `acknowledgements_are_the_shells_bodies` in crates/coordinator/tests/index_admin.rs
- AC-6: `a_failed_alias_after_a_create_says_the_index_exists` in crates/coordinator/tests/index_admin.rs
