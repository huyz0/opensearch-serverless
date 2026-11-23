# Verified

Run on the in-process emulator over the recorded shell store, so the request numbers are provisional.

- AC-1: `a_rust_writer_takes_a_java_shard_and_keeps_its_blobs` in crates/node/tests/mixed_takeover.rs. It asserts the next sequence number, term 3 and the new owner on the head; it does not count documents.
- AC-2: `the_commit_a_rust_writer_publishes_still_names_the_java_blobs` in the same file; the commit register names the Java node pack and segment blob.
- AC-3: `a_live_janitor_sweep_after_the_take_deletes_no_blob_the_commit_names` in the same file; a planted older-term blob is swept, so the pass ran, and the Java blobs stay.

Not covered: a Java node reading the Rust-published commit of this shard (the R6 exit gate does it for a Rust-written shard), a Java node's node log taken over by Rust (the node-log replay fixtures cover the log), and the janitor's dry-run default, which this evidence supports but does not flip.
