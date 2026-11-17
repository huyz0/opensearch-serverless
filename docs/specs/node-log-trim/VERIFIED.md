# Node log trimming: verification

- AC-1: `an_epoch_and_a_renewal_each_write_one_lease_naming_inherited_epochs_first` (writer, `tests/node_log_keeper.rs`).
- AC-2: `a_renewal_deletes_below_the_trim_point_in_one_request_and_then_nothing` (writer, on the emulator's meter).
- AC-3: `a_complete_epoch_is_left_out_of_the_lease_then_deleted_whole` (writer).
- AC-4: `a_failed_lease_write_deletes_nothing_and_a_failed_delete_is_retried`, `a_complete_epoch_whose_delete_failed_is_deleted_at_the_next_renewal` (writer).
- AC-5: `inherited_epochs_are_named_in_every_lease_and_never_deleted` (writer).
- AC-6: `a_renewal_racing_new_epochs_never_drops_one_a_blob_landed_in` (writer).
