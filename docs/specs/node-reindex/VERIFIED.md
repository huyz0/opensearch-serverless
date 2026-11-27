# Reindex between two indexes, with an optional Painless script: verified

Run 2026-11-27 in a Linux cloud sandbox (emulator store, so numbers are provisional). Coverage and mutants were not run in the sandbox.

- AC-1: `matching_documents_are_copied_to_the_destination` in crates/node/tests/server/reindex_route.rs
- AC-2: `a_script_edits_skips_or_deletes_in_the_destination` in crates/node/tests/server/reindex_route.rs
- AC-3: `op_type_create_conflicts_on_an_existing_document_and_index_overwrites` in crates/node/tests/server/reindex_route.rs
- AC-4: `reindex_max_docs_bounds_the_copy` in crates/node/tests/server/reindex_route.rs
- AC-5: `a_reindex_this_node_cannot_serve_is_refused` in crates/node/tests/server/reindex_route.rs
