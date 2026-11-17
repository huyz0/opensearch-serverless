# Shard engine: verification

- AC-1: `document_steps_answer_as_the_java_does` (writer, `tests/document_fixtures.rs`, over `fixtures/compat/documents/*.json`).
- AC-2: `updates_and_deletes_soft_delete_as_the_java_does` (writer, `tests/segment_fixtures.rs`, over `fixtures/segments/updates.json`).
- AC-3: `a_version_is_found_in_the_map_or_in_a_segment` (writer, `tests/shard.rs`); `document_steps_answer_as_the_java_does` replays every step a second time with a refresh after each.
- AC-4: `a_refused_write_takes_no_sequence_number_and_changes_nothing` (writer, `tests/shard.rs`).
