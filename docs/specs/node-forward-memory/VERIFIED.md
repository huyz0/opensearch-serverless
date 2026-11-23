# Verified: node-forward-memory

- AC-1: `a_borrowed_request_writes_the_bytes_of_an_owned_one_and_the_hint_covers_them` in crates/transport/tests/wire/forwarding.rs (and the recorded Java bytes in `every_recorded_forwarded_write_is_the_javas_both_ways`)
- AC-2: `forwarding_a_large_group_holds_its_sources_about_once` in crates/node/tests/forward_peak.rs
- AC-3: `a_group_is_applied_in_order_and_each_outcome_is_the_engines` in crates/node/tests/roles/bulk_handler.rs, unchanged
- AC-4: `a_borrowed_request_writes_the_bytes_of_an_owned_one_and_the_hint_covers_them` in crates/transport/tests/wire/forwarding.rs

Measured (debug build, counting allocator): the old shape (owned copy of the request, buffer growing from empty) peaked at 50,332,868 bytes over a 16,777,216-byte group, about 3.0 times; the new shape stays within 1.25 times. The old shape was simulated in the test's sender, since the old API no longer compiles.

NOT-RUN: coverage (no cargo-llvm-cov in the sandbox); the owner-side saving as a peak (one fewer clone by construction); `signed_call` end to end over a real transport for a group this large.
