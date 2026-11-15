# Registers: verified

Run with `cargo test -p oss-blob --features emulator --test registers` and the whole gate, `scripts/gate.sh`, on 2026-11-15 (Linux sandbox, ext4-backed temporary directory, OpenJDK on the path for the manual check). Each listed defect was planted and seen to fail: the emulator ignoring `If-Match` (`each_operation_sends_what_the_spec_budgets`, `a_stale_remembered_version_costs_one_refused_put_never_a_wrong_write`), the client dropping `If-Match` (the same), a remembered ETag used for another generation (`a_register_is_created_swapped_and_deleted_by_generation`), a create that reads first (`what_does_not_fit_is_refused_never_truncated`, `each_operation_sends_what_the_spec_budgets`), and the fs lock taken with `flock` instead of `fcntl` (`a_swap_waits_for_another_processes_posix_lock`).

Manual JVM check, once, not in the gate: `a_swap_waits_for_another_processes_posix_lock` with its child replaced by a Java program holding `FileChannel.lock()` on the same stripe file passed (the swap waited, then applied), and failed with the `flock` defect planted; the stripe values in `stripes_match_javas_string_hash_code` were printed by a JVM's `Math.floorMod(s.hashCode(), 64)`.

- AC-1: `a_register_is_created_swapped_and_deleted_by_generation`, `a_register_is_its_generation_then_its_value`, `initial_generations_are_in_the_javas_range`, `a_generation_steps_by_one_within_the_signed_range`
- AC-2: `one_of_racing_swaps_applies_and_its_value_is_stored`, `one_of_racing_creates_applies_and_its_value_is_stored`
- AC-3: `each_operation_sends_what_the_spec_budgets`
- AC-4: `a_stale_remembered_version_costs_one_refused_put_never_a_wrong_write`, `two_clients_swap_one_register_without_losing_a_write`, `a_swap_of_a_deleted_register_is_a_conflict`, `holds_at_most_its_cap_forgetting_the_oldest`
- AC-5: `a_swap_whose_response_was_lost_finds_its_own_write`, `a_create_refused_on_retry_by_another_writers_register_is_a_conflict`
- AC-6: `a_register_the_java_shell_wrote_reads_with_its_generation`
- AC-7: `a_swap_waits_for_another_processes_posix_lock`, `stripes_match_javas_string_hash_code`, `registers_in_one_directory_use_their_own_stripes`
- AC-8: `what_does_not_fit_is_refused_never_truncated`, `a_decoder_refuses_what_does_not_fit_or_is_too_short`, `a_source_yields_the_object_through_any_buffer_and_rewinds`
- AC-9: `scripts/check-alloc.py`
