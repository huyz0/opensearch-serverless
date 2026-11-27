# Verified: writer-restore

- AC-1: `a_shard_opened_over_the_restored_commit_holds_its_documents_and_carries_on` in crates/writer/tests/it/restore.rs
- AC-2: `every_file_of_the_commit_is_restored_with_its_bytes` in crates/writer/tests/it/restore.rs
- AC-3: `a_restore_reads_the_register_once_and_each_blob_once` in crates/writer/tests/it/restore.rs
- AC-4: `a_shard_with_no_register_restores_nothing_and_reads_no_blob` in crates/writer/tests/it/restore.rs
- AC-5: `a_blob_that_ends_before_its_files_do_is_refused` in crates/writer/tests/it/restore.rs
