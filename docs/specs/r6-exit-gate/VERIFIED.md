# R6 exit gate: verification

- AC-1: run by hand on 2026-11-18, `java.txt` is its output (term 3, writer node-1, 43 files, `num_docs 2`, `max_doc 5`); see `fixtures/exit-gate/README.md`. Held by `the_rust_reader_counts_what_the_java_reader_counted_in_a_rust_written_commit` (node, `tests/roles/exit_gate.rs`), which reads the recorded numbers.
- AC-2: `the_rust_reader_counts_what_the_java_reader_counted_in_a_rust_written_commit` (node, `tests/roles/exit_gate.rs`).
- AC-3: `the_reader_node_answers_every_search_fixture_as_opensearch_did` (compat, `tests/node_search.rs`) and `the_commit_is_found_with_two_gets_and_no_list` (reader).
- AC-4: `bulk_steps_answer_as_stock_opensearch_does`, `document_steps_answer_as_the_java_does`, `mapping_steps_replay`, `rust_documents_index_as_the_java_indexes_them` (writer, `tests/*_fixtures.rs`).
