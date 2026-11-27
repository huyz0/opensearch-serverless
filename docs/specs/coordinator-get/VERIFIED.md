# Verified: the coordinator decides where a get is served and whether its version holds

- AC-1: `a_shard_held_here_and_owned_here_or_by_nobody_is_read_here` in crates/coordinator/tests/it/get.rs
- AC-2: `a_shard_held_here_but_owned_elsewhere_goes_to_its_owner` in crates/coordinator/tests/it/get.rs
- AC-3: `with_no_owner_nothing_published_is_absent_and_a_commit_is_read` in crates/coordinator/tests/it/get.rs
- AC-4: `another_nodes_shard_is_forwarded_to_it` in crates/coordinator/tests/it/get.rs
- AC-5: `a_shard_owned_here_but_not_held_is_refused_as_acquiring` in crates/coordinator/tests/it/get.rs
- AC-6: `a_pinned_version_conflicts_as_the_shell_says` in crates/coordinator/tests/it/get.rs
- AC-6 (the markers): `the_version_markers_are_the_shells_numbers` in crates/coordinator/tests/it/get.rs
