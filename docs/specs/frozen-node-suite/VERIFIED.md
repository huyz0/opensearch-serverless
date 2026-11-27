# Frozen-node suite: verification

Run in the cloud sandbox with `cargo test -p oss-node --features reader,writer,coordinator --test frozen_node`. The cluster scenario was first red on its own timing: a survivor write forwarded to the frozen owner waited the 30 s forward timeout, which led to the two-TTL wait in the spec.

- AC-1: `a_frozen_freezer_holds_bytes_and_delivers_them_whole_and_in_order_on_thaw` (node, `tests/frozen_node.rs`).
- AC-2: `a_write_held_at_a_frozen_owner_is_counted_once_or_not_at_all_after_the_thaw` (node, `tests/frozen_node.rs`).
- AC-3: `a_write_held_at_a_frozen_owner_is_counted_once_or_not_at_all_after_the_thaw` (node, `tests/frozen_node.rs`).
- AC-4: `a_write_held_at_a_frozen_owner_is_counted_once_or_not_at_all_after_the_thaw` (node, `tests/frozen_node.rs`).
- AC-5: `a_write_held_at_a_frozen_owner_is_counted_once_or_not_at_all_after_the_thaw` (node, `tests/frozen_node.rs`). The head is asserted on its own: `cluster.owner_slot` names a survivor while the owner is frozen and after the thaw and the write through the thawed node.
- AC-6: `a_process_stopped_with_sigstop_loses_its_shard_and_doubles_nothing_on_resume` (node, `tests/frozen_process.rs`): the owner is the `oss-node` binary as its own process, stopped with `kill -STOP` past two lease TTLs and resumed with `kill -CONT`, over two in-process nodes on the emulated store (3 s TTL, cloud container, emulator only, provisional). Survivors dropped it from their view, a survivor took the shard and the head named it; a write sent to the stopped process waited in its socket; after the resume every node counts the acknowledged documents once and the held write's documents once if it was acknowledged and not at all if refused; the head stayed with the survivor. Passed on its first run (15 s); seen red with the survivor expectation planted wrong (`head names n2, not a survivor`).

Not run: RustFS for these two tests (the emulator is the store; the rig's `Cluster::new` takes RustFS but `owner_id` and `owner_slot` read the emulator).
