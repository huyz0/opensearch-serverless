# Store usage: verification

- AC-1: `a_request_is_charged_to_the_index_its_key_names`, `every_kind_of_call_is_charged_to_its_keys_index`, `a_batch_delete_is_the_index_its_keys_all_name_or_the_nodes` (blob, `tests/emulator/usage.rs`).
- AC-2: `the_lines_add_up_to_what_the_meter_counted` (blob, `tests/emulator/usage.rs`).
- AC-3: `bytes_are_charged_to_the_index_that_moved_them` (blob, `tests/emulator/usage.rs`).
- AC-4: `a_shared_upload_is_split_by_weight_and_the_heaviest_takes_the_remainder`, `a_split_with_no_weight_charges_the_node_and_a_key_beats_a_split` (blob, `tests/emulator/usage.rs`).
- AC-5: `indices_past_the_limit_are_counted_together` (blob, `tests/emulator/usage.rs`).
- AC-6: `the_price_of_a_line_is_a_put_a_get_at_eight_hundredths_and_a_free_delete` (blob, `tests/emulator/usage.rs`).
- AC-7: `keys_that_name_an_index_give_it`, `keys_no_single_index_owns_give_none` (format, `tests/key_owner.rs`).

## Write cost, for the default (provisional)

Run by hand: `cargo test -p oss-node --all-features --test store_usage_measure -- --ignored --nocapture` (crates/node/tests/store_usage_measure.rs): a debug build on 4 cores over a file store, 8 clients each sending bulks of 20 documents to an index of its own, 4 alternating rounds of 6 s with `OSS_STORE_USAGE` off and on. The usage adds no store request (it counts the ones made). Three full runs, after the write-concurrency fix:

| Run | Usage off | Usage on |
|---|---|---|
| 1 | 400 bulks, p50 508.0 ms, p99 599.9 ms | 384 bulks, p50 508.5 ms, p99 1207.0 ms |
| 2 | 400 bulks, p50 508.2 ms, p99 521.8 ms | 400 bulks, p50 508.1 ms, p99 517.1 ms (-1%) |
| 3 | 400 bulks, p50 507.9 ms, p99 515.6 ms | 400 bulks, p50 508.6 ms, p99 530.2 ms (+3%) |

Run 1 had a slow stretch with usage on (one round) and runs 2 and 3 do not repeat it, so the cost on p99 is not established either way: two runs are within 5% and one is not. The default stays off until a run repeats clean. Before the write-concurrency fix every write on a node waited alone for its own commit window, which hid any such cost (the same test then measured about 120 bulks, p50 2,034 ms, with either setting).

### Six more runs, after the pipeline work (provisional)

The same command on the same setup, now at about 254 ms a bulk (one window), 768 bulks a leg (the earlier runs were slower because every write waited for its own window):

| Run | Usage off p99 | Usage on p99 | Change |
|---|---|---|---|
| 1 | 268.0 ms | 302.5 ms | +12.9% |
| 2 | 261.9 ms | 265.8 ms | +1.5% |
| 3 | 260.5 ms | 267.6 ms | +2.7% |
| 4 | 260.2 ms | 263.0 ms | +1.1% |
| 5 | 262.3 ms | 261.5 ms | -0.3% |
| 6 | 260.7 ms | 262.6 ms | +0.7% |

The p50 is the same in every run (254.0 to 254.3 ms) and no write was rejected. Run 1 is the first after a build and, like the earlier slow stretch, ran one round long (usage on, max 332.8 ms); runs 2 to 6 repeat clean, five in a row within 5% (median +1.1%). Taking the earlier three runs too, seven of nine are within 5%, and the two that are not were the first run after a build, with the slow stretch not reproduced by any later run. The default is on: the cost is two atomic adds and a thread-local read a store attempt, measured within 5% of write p99, with no added request. `OSS_STORE_USAGE=false` turns it off, and both settings are tested.
