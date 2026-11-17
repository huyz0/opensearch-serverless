# Node log write-ack meter: verification

- AC-1: `one_caller_is_one_blob_one_epoch_and_waits_out_its_window` (writer, `tests/node_log_ack_meter.rs`).
- AC-2: `thirty_callers_in_one_window_are_one_put` (writer).
- AC-3: `a_caller_behind_a_slow_blob_counts_the_wait_behind_it` (writer).
- AC-4: `a_reset_starts_the_sums_over_and_the_line_names_each` (writer).
