# Verified

- AC-1: `only_the_ranked_readers_of_another_build_are_told_of_every_commit` in crates/node/tests/tail/push_link.rs
- AC-2: `without_the_send_every_commit_still_goes_to_the_readers_of_another_build` in crates/node/src/commit_push/tests.rs (seen failing with the new send removed)
- AC-3: measured once on RustFS (provisional): cycle 1 (Java killed) and cycle 2 (Rust killed) passed with every Java and Rust reader seeing all acknowledged documents; before this change Java readers refused the shard in cycle 2. Cycle 3 then failed on a Rust reader missing 152 of 740 recent acknowledged documents, a separate gap this change does not address. Single run, `soak_wide` (ignored test).
