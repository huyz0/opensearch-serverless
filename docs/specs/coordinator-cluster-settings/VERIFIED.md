# Verified: the coordinator decides the cluster settings calls

- AC-1: `a_body_needs_a_persistent_object` in crates/coordinator/tests/cluster_settings.rs
- AC-2: `nothing_to_update_is_a_validation_failure` in crates/coordinator/tests/cluster_settings.rs
- AC-3: `a_transient_setting_is_refused_first` in crates/coordinator/tests/cluster_settings.rs
- AC-4: `persistent_settings_come_back_flat_and_sorted` in crates/coordinator/tests/cluster_settings.rs
- AC-5: `a_list_is_refused_by_the_first_key` in crates/coordinator/tests/cluster_settings.rs
- AC-6: `the_named_settings_are_checked` in crates/coordinator/tests/cluster_settings.rs
- AC-7: `the_checkpoint_age_is_bounded_by_the_commit_period` in crates/coordinator/tests/cluster_settings.rs
- AC-8: `the_plugin_check_runs_last` in crates/coordinator/tests/cluster_settings.rs
- AC-9: `the_bodies_nest_or_stay_flat` in crates/coordinator/tests/cluster_settings.rs
