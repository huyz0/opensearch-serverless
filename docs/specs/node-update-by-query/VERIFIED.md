# Update by query with a Painless script: verified

Run 2026-11-27 in a Linux cloud sandbox (emulator store, so numbers are provisional). Coverage and mutants were not run in the sandbox.

- AC-1: `a_script_rewrites_deletes_or_skips_the_matching_documents` in crates/node/tests/server/by_query_route.rs
- AC-2: `a_script_rewrites_deletes_or_skips_the_matching_documents` in crates/node/tests/server/by_query_route.rs
- AC-3: `an_update_conflicts_on_a_rewritten_document_and_stops_at_max_docs` in crates/node/tests/server/by_query_route.rs
- AC-4: `an_update_conflicts_on_a_rewritten_document_and_stops_at_max_docs` in crates/node/tests/server/by_query_route.rs
- AC-5: `an_update_by_query_this_node_cannot_serve_is_refused` in crates/node/tests/server/by_query_route.rs
