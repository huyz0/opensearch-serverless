# Store usage of the write log: verification

- AC-1: `a_blobs_put_is_shared_by_the_bytes_of_each_indexs_slices` (writer, `tests/node_log_usage.rs`).
- AC-2: `a_hedged_put_is_shared_too_though_it_runs_on_another_thread` (writer, `tests/node_log_usage.rs`).
- AC-3: `a_blob_whose_slices_name_no_index_is_the_nodes` (writer, `tests/node_log_usage.rs`).
- AC-4: `a_log_without_usage_charges_nothing_to_an_index` (writer, `tests/node_log_usage.rs`).
