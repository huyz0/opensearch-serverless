# Blob batch delete: verification

- AC-1: `a_batch_delete_is_one_request_a_thousand_keys` (blob, `tests/emulator/deletes.rs`); `a_batch_delete_is_one_post_a_thousand_keys_on_client_and_server` (blob, `tests/s3/deletes.rs`).
- AC-2: `a_batch_delete_checks_every_key_before_sending_anything` (blob, `tests/emulator/deletes.rs`); `a_batch_delete_checks_every_key_before_any_request` (blob, `tests/s3/deletes.rs`).
- AC-3: `a_failed_batch_leaves_the_keys_after_it_and_deleting_again_finishes` (blob, `tests/emulator/deletes.rs`); `a_failed_batch_is_retried_and_each_attempt_counted` (blob, `tests/s3/deletes.rs`).
- AC-4: `a_batch_delete_is_one_post_a_thousand_keys_on_client_and_server` (blob, `tests/s3/deletes.rs`); `base64_matches_rfc_4648_vectors`, `the_body_names_each_key_escaped_and_asks_for_quiet`, `an_answer_listing_no_error_is_success_however_it_arrives`, `the_first_error_listed_is_the_failure_wherever_it_falls`, `an_answer_that_breaks_off_is_unavailable` (unit, `s3/delete.rs`).
- AC-5: `a_batch_delete_on_disk_is_one_delete_a_key_after_checking_them_all` (blob, `tests/fs/writes.rs`).
- AC-6: `the_client_round_trips_against_rustfs` (blob, `tests/rustfs.rs`). It skips without `RUSTFS_BIN`, and it has not run here because this environment cannot download the RustFS binary.
- AC-7: `a_batch_delete_is_a_round_a_thousand_keys` (blob, `tests/rounds.rs`).
- AC-8: `deletes_go_by_name` (writer, `tests/node_log_store.rs`).
