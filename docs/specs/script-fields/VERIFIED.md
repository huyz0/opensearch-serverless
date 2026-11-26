# `script_fields` in the expression language: verified

Run 2026-11-26 in a Linux cloud sandbox (emulator store, so numbers are provisional): `cargo test -p oss-node --all-features --test it -- script_fields`.

- AC-1: `a_script_field_is_written_for_each_hit` in crates/node/tests/it/script_fields.rs
- AC-2: `a_script_field_sits_beside_docvalue_fields` in crates/node/tests/it/script_fields.rs
- AC-3: `too_many_script_fields_are_refused` in crates/node/tests/it/script_fields.rs
- AC-4: `what_is_not_served_is_refused` in crates/node/tests/it/script_fields.rs
- AC-5: `a_script_field_adds_no_store_request` in crates/node/tests/it/script_fields.rs

Not run: output against a recorded Java OpenSearch, and `script_fields` inside inner hits or `top_hits` (refused because those parsers do not read the key).
