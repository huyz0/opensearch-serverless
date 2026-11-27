# Verified: a writer node serves `POST /{index}/_update/{id}`

Run on 2026-11-27 in a Linux sandbox: `cargo test -p oss-node --all-features --test server update_route`.

- AC-1: `a_doc_merges_into_the_stored_document` in crates/node/tests/server/update_route.rs
- AC-2: `a_doc_merges_into_the_stored_document`, `detect_noop_false_writes_an_unchanged_merge` in crates/node/tests/server/update_route.rs
- AC-3: `a_missing_document_is_refused_or_upserted` in crates/node/tests/server/update_route.rs
- AC-4: `an_update_script_edits_deletes_or_skips_behind_the_switch` in crates/node/tests/server/update_route.rs
- AC-5: `the_callers_condition_and_the_options_are_checked` in crates/node/tests/server/update_route.rs
