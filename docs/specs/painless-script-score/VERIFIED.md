# `function_score` script_score in Painless: verified

Run 2026-11-27 in a Linux cloud sandbox (emulator store, so numbers are provisional).

- AC-1: `painless_is_served_only_while_the_switch_is_on` (crates/reader/src/dsl/queries/function_score_tests.rs), `painless_source_is_refused_unless_its_variable_turns_the_compiler_on` (crates/node/src/boot/config/tests/switches.rs), and `what_is_not_served_is_refused` in crates/node/tests/it/function_score_script.rs with the switch off.
- AC-2: `painless_scores_like_the_same_expression` in crates/node/tests/script_score_painless.rs
- AC-3: `a_script_reads_the_score_and_declares_variables` in crates/node/tests/script_score_painless.rs
- AC-4: `what_painless_rejects_or_this_node_cannot_run_is_refused` in crates/node/tests/script_score_painless.rs and `a_scripts_exception_is_a_script_exception_with_its_cause` in crates/reader/src/script_failure.rs
- AC-5: `a_painless_script_adds_no_store_request` in crates/node/tests/script_score_painless.rs
- AC-6: `a_painless_script_field_is_written_for_each_hit` in crates/node/tests/script_score_painless.rs
- AC-7: `a_painless_script_sorts_hits` in crates/node/tests/script_score_painless.rs

Not run: scores against a recorded Java OpenSearch; a keyword field read by a script (the engine's kind is covered in lucene-rust, not over this node's mapping).
