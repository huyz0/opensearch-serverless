# Node log write-ack meter: verification

- AC-1: `one_caller_is_one_blob_one_epoch_and_waits_out_its_window` (writer, `tests/node_log_ack_meter.rs`).
- AC-2: `thirty_callers_in_one_window_are_one_put` (writer).
- AC-3: `a_caller_behind_a_slow_blob_counts_the_wait_behind_it` (writer).
- AC-4: `a_reset_starts_the_sums_over_and_the_line_names_each`, `means_are_sums_over_counts_in_milliseconds` (writer).
- AC-5: `open_load_ack_meter` (writer, `tests/node_log_open_load.rs`, run with `OSS_ACK_SECONDS=30 cargo test --release -p oss-writer --test node_log_open_load -- --ignored --nocapture`). Two runs on 2026-11-17, provisional (emulator, 4-core cloud container): p50 150.5 and 149.2 ms, p90 254 and 255 ms, p99 271 ms. Time per caller: 129 ms waiting for the window, 0.2 ms behind the blob in flight, 20.5 ms for its own write (PUT 20.2 ms). The shell's open mode measured p50 158 ms, of which 131 ms was the window and 25 ms the PUT (serverless/README.md). This run covers only the log: there is no REST or indexing step yet, and no 200 ms tail on every 50th request as the shell's store injects.
