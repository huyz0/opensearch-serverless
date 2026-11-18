# Writer soak: verification

- AC-1: `a_thousand_shards_publish_with_the_requests_of_one_blob_and_one_swap_each` (writer, `tests/writer_soak.rs`): 3,000 publishes, 6,000 PUT, 0 LIST. Emulator, provisional.
- AC-2: `a_thousand_shards_publish_with_the_requests_of_one_blob_and_one_swap_each` (writer, `tests/writer_soak.rs`): peak heap 85,752,216 B measured, bound 128 MiB. Debug build, one machine, provisional.
