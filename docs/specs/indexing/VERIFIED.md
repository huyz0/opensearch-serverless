# Indexing: verification

- AC-1: `rust_documents_index_as_the_java_indexes_them` (writer, `tests/segment_fixtures.rs`, over `fixtures/segments/*.json`): the field infos the engine wrote, read back, against the recorded ones.
- AC-2: `rust_documents_index_as_the_java_indexes_them` (writer, `tests/segment_fixtures.rs`): every document's stored fields, terms, norms, doc values and points.
- AC-3: `rust_documents_index_as_the_java_indexes_them` over `fixtures/segments/ids.json` (writer); `ids_encode_as_uid_encodes_them` (writer, `src/lucene/encode.rs`).
- AC-4: `rust_documents_index_as_the_java_indexes_them` (writer): one committed segment of the recorded document count.
- AC-5: `a_later_segment_keeps_the_writers_field_numbers` and `a_mapping_update_the_schema_cannot_take_is_refused` (writer, `tests/lucene_fields.rs`).
