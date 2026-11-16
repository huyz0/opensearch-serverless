# Settings registry: verified

Run on 2026-11-16 in a Linux sandbox:
- `cargo test -p oss-types`, `cargo test -p oss-format --test cluster_settings`, `cargo test -p oss-blob --lib settings`
- `cargo test -p oss-node --lib settings` with the default features, with none, with `writer` alone and with `coordinator` alone
- `cargo mutants` over the diff in `oss-types`, `oss-format`, `oss-blob` and `oss-node`: 162 mutants, 0 missed
- the whole gate, `scripts/gate.sh`

An independent review found that resolution refused an index or cluster value that does not parse, where the Java's `LayeredSetting` skips it, so Rust and Java nodes on one bucket would have decided differently. It also found that an `index.serverless.` key passed a cluster update, that the refusals were not the Java's text, and that a writer-only or coordinator-only build had no store settings. Each is fixed and tested.

- AC-1: `a_bool_is_true_or_false_and_nothing_else`, `numbers_parse_within_their_bounds_with_the_java_messages`, `a_choice_is_one_of_its_list`, `an_absent_key_is_the_default_and_a_present_one_is_never_trimmed`
- AC-2: `index_overrides_cluster_overrides_node`, `an_empty_value_is_unset_at_every_layer`, `a_bad_value_is_skipped_and_the_next_layer_decides`
- AC-3: `a_registry_refuses_a_key_defined_twice`, `a_registry_refuses_a_key_outside_its_prefix_or_without_a_doc`, `a_registry_refuses_a_default_it_would_refuse_as_a_value`, `a_registry_finds_keys_and_reports_unknown_ones`, `an_update_is_checked_against_the_registry`, `an_index_setting_is_no_cluster_update`
- AC-4: `live_settings_take_only_newer_generations`, `a_snapshot_outlives_the_swap`
- AC-5: `fs_is_the_default_and_needs_a_path`, `s3_needs_a_bucket_and_defaults_the_rest`, `cluster_settings_are_a_source`, `the_nodes_settings_form_a_valid_registry`

Each planted defect was seen to fail:

| Planted defect | Test that failed |
|---|---|
| a bad cluster value refused instead of skipped | `a_bad_value_is_skipped_and_the_next_layer_decides` |
| a bad index value refused instead of skipped | `a_bad_value_is_skipped_and_the_next_layer_decides` |
| `Live` applying a snapshot of the same generation | `live_settings_take_only_newer_generations` |
| a defined index setting accepted as a cluster update | `an_index_setting_is_no_cluster_update` |
| a key that is not dynamic accepted as a cluster update | `an_update_is_checked_against_the_registry` |
| the register's list read as its first value | `cluster_settings_are_a_source` |
| the store type defaulting to `s3` | `fs_is_the_default_and_needs_a_path` |
| one crate's settings collected twice | `the_nodes_settings_form_a_valid_registry` |
