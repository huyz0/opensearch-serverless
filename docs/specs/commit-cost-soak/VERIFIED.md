# Verified

Emulator with a latency profile, in-process: counts are exact for the run and any timing is provisional. The decision is in the spec; no default is flipped.

- AC-1: `the_commit_switches_cost_no_more_than_the_defaults` in crates/node/tests/commit_cost.rs
- AC-2: `the_commit_switches_cost_no_more_than_the_defaults` in crates/node/tests/commit_cost.rs
- AC-3: `the_commit_switches_cost_no_more_than_the_defaults` in crates/node/tests/commit_cost.rs
- AC-4: `the_commit_switches_cost_no_more_than_the_defaults` in crates/node/tests/commit_cost.rs (the opening bound was loosened from exact to within a tenth and four requests on 2026-11-26, on Huy's decision: one CI run measured 557 against 555)
- AC-5: `the_commit_switches_cost_no_more_than_the_defaults` in crates/node/tests/commit_cost.rs
