# Store usage: verification

- AC-1: `a_request_is_charged_to_the_index_its_key_names`, `every_kind_of_call_is_charged_to_its_keys_index`, `a_batch_delete_is_the_index_its_keys_all_name_or_the_nodes` (blob, `tests/emulator/usage.rs`).
- AC-2: `the_lines_add_up_to_what_the_meter_counted` (blob, `tests/emulator/usage.rs`).
- AC-3: `bytes_are_charged_to_the_index_that_moved_them` (blob, `tests/emulator/usage.rs`).
- AC-4: `a_shared_upload_is_split_by_weight_and_the_heaviest_takes_the_remainder`, `a_split_with_no_weight_charges_the_node_and_a_key_beats_a_split` (blob, `tests/emulator/usage.rs`).
- AC-5: `indices_past_the_limit_are_counted_together` (blob, `tests/emulator/usage.rs`).
- AC-6: `the_price_of_a_line_is_a_put_a_get_at_eight_hundredths_and_a_free_delete` (blob, `tests/emulator/usage.rs`).
- AC-7: `keys_that_name_an_index_give_it`, `keys_no_single_index_owns_give_none` (format, `tests/key_owner.rs`).
