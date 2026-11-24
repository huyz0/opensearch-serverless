# Template admin decisions: verified

Run on 2026-11-24 in a Linux sandbox: `cargo test -p oss-coordinator --test template_admin`, `cargo clippy -p oss-coordinator --all-targets -- -D warnings`.

The expectations come from reading the Java source, not a recorded run.

- AC-1: `an_index_template_needs_patterns_and_a_component_must_not_have_them`
- AC-2: `a_body_that_is_not_one_object_is_malformed`
- AC-3: `aliases_replicas_and_bad_shapes_are_refused_not_stored`
- AC-4: `a_mutation_names_one_concrete_template`
- AC-5: `a_name_selects_one_a_prefix_or_all`, `a_get_answers_the_name_and_the_definition_under_the_sets_key`
