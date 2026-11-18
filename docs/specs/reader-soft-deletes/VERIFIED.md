# Reader soft deletes: verification

- AC-1: `a_replaced_or_deleted_document_is_not_found` (node, `tests/push_read.rs`).
- AC-2: `a_push_moves_the_reader_onto_the_new_commit_with_one_get` (node, `tests/push_read.rs`, an index with no deletes) and the oss-reader and oss-node suites over the recorded indexes, run green with the change.
