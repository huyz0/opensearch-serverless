# Frozen-node suite: verification

Run in the cloud sandbox with `cargo test -p oss-node --features reader,writer,coordinator --test frozen_node`. The cluster scenario was first red on its own timing: a survivor write forwarded to the frozen owner waited the 30 s forward timeout, which led to the two-TTL wait in the spec.

- AC-1: `a_frozen_freezer_holds_bytes_and_delivers_them_whole_and_in_order_on_thaw` (node, `tests/frozen_node.rs`).
- AC-2: `a_write_held_at_a_frozen_owner_is_counted_once_or_not_at_all_after_the_thaw` (node, `tests/frozen_node.rs`).
- AC-3: `a_write_held_at_a_frozen_owner_is_counted_once_or_not_at_all_after_the_thaw` (node, `tests/frozen_node.rs`).
- AC-4: `a_write_held_at_a_frozen_owner_is_counted_once_or_not_at_all_after_the_thaw` (node, `tests/frozen_node.rs`).
- AC-5: `a_write_held_at_a_frozen_owner_is_counted_once_or_not_at_all_after_the_thaw` (node, `tests/frozen_node.rs`). NOT-RUN: the head held by one node is not asserted separately; the successor's acknowledged writes and the fenced refusal show it.
