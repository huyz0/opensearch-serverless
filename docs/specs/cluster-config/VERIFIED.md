# Cluster config: verified

Run on 2026-11-16 in a Linux sandbox:
- `cargo test -p oss-format --test cluster_settings`
- `cargo test -p oss-meta`
- the whole gate, `scripts/gate.sh`

The fixtures were written by running `ClusterSettings.java` on OpenJDK 21 against the OpenSearch 3.3.0 jars (`fixtures/cluster-config/README.md`).

`cargo mutants` over the diff left no mutant missed.

Each planted defect was seen to fail:

| Planted defect | Test that failed |
|---|---|
| an update that swaps when it should create, and the reverse | `updates_create_then_swap_and_merge`, `a_value_a_java_node_stored_is_read_and_updated`, `an_update_retries_while_others_move_the_register` |
| 9 attempts instead of 10 | `an_update_retries_while_others_move_the_register` |
| a conflicting update written anyway | `an_update_the_java_would_refuse_writes_nothing` |
| no check for a `p.0` left beside a `p` | `every_case_reads_as_the_java_reads_it`, `an_update_the_java_refuses_changes_nothing` |
| a list entry joined without the Java's `, ` | `every_case_reads_as_the_java_reads_it` |
| list entries not removed once in the list | `every_case_reads_as_the_java_reads_it`, `an_update_the_java_refuses_changes_nothing` |
| an update that builds a list let through | `an_update_that_makes_a_list_is_refused` |
| lists already held dropped by an update | `lists_already_held_survive_an_unrelated_update` |
| no bound while reading, or a bound that counts a nested key once | `settings_past_the_bound_are_refused` |
| `apply`'s bound one byte loose | `settings_past_the_bound_are_refused` |
| `null` read as an empty value | `every_case_reads_as_the_java_reads_it` |

Evidence for each criterion:

- AC-1: `every_case_reads_as_the_java_reads_it`, `the_javas_nested_writes_read_as_its_flat_form`, `values_round_trip_with_escapes`, `a_short_buffer_says_how_long_it_must_be`, `malformed_values_are_refused`, `the_key_is_the_javas`
- AC-2: `updates_merge_as_the_javas`, `an_update_that_makes_a_list_is_refused`, `lists_already_held_survive_an_unrelated_update`, `an_update_the_java_refuses_changes_nothing`, `settings_past_the_bound_are_refused`
- AC-3: `nothing_set_reads_empty_in_one_request`, `updates_create_then_swap_and_merge`, `an_update_retries_while_others_move_the_register`, `an_update_the_java_would_refuse_writes_nothing`, `a_value_too_long_for_the_buffer_is_refused`
- AC-4: `a_value_a_java_node_stored_is_read_and_updated`, `a_malformed_value_or_base_is_refused`
