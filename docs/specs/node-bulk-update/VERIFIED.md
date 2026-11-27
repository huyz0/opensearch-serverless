# Verified: a bulk `update` item is served

Run on 2026-11-27 in a Linux sandbox: `cargo test -p oss-writer --test it bulk_` and `cargo test -p oss-node --all-features --lib --test server`.

- AC-1: `bulk_updates_merge_skip_upsert_and_refuse` in crates/node/tests/server/bulk_update.rs, `an_update_is_applied_as_the_write_it_resolved_to` in crates/writer/tests/it/bulk_update.rs
- AC-2: `bulk_updates_merge_skip_upsert_and_refuse` in crates/node/tests/server/bulk_update.rs
- AC-3: `a_bulk_update_script_edits_deletes_or_is_refused_without_the_switch` in crates/node/tests/server/bulk_update.rs
- AC-4: `a_second_item_for_one_document_in_a_slice_is_a_conflict` in crates/writer/tests/it/bulk_update.rs, `two_updates_of_one_document_in_one_bulk_conflict` in crates/node/tests/server/bulk_update.rs
- AC-5: `bulk_updates_merge_skip_upsert_and_refuse` in crates/node/tests/server/bulk_update.rs
- AC-6: `without_updates_an_update_item_is_not_supported` in crates/writer/tests/it/bulk_update.rs
