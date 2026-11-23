# Verified

Run on the in-process emulator over the recorded shell store, so the request numbers are provisional.

- AC-1: `a_rust_writer_takes_a_java_shard_and_keeps_its_blobs` in crates/node/tests/mixed_takeover.rs. It asserts the next sequence number, term 3 and the new owner on the head; it does not count documents.
- AC-2: `the_commit_a_rust_writer_publishes_still_names_the_java_blobs` in the same file; the commit register names the Java node pack and segment blob.
- AC-3: `a_live_janitor_sweep_after_the_take_deletes_no_blob_the_commit_names` in the same file; a planted older-term blob is swept, so the pass ran, and the Java blobs stay.

- AC-4: `the_rust_reader_counts_what_the_java_reader_counted_in_the_commit_over_a_java_pack` in crates/node/tests/mixed_java_reads.rs against `java.txt`: 6 live documents, 3 files in the Java node pack. Recorded once from the shell at 334bd9bc; the test fails if the recorded count is changed.
- AC-5: `the_commit_and_head_read_as_the_java_read_them` in the same file for the head the Java read; the take at term 4 is in `java.txt` only (`acquired true term 4`), not re-run in CI.

Not covered: a mixed fleet running live (the Java side is a recording over the filesystem layout, with the server's mappers and blob store stubbed), a Java node's node log taken over by Rust (the node-log replay fixtures cover the log), and the janitor's dry-run default, which this evidence supports but does not flip.
