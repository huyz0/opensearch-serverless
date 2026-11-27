# `scripted_metric`: verified

Run 2026-11-27 in a Linux cloud sandbox (emulator store, so numbers are provisional). Coverage and mutants were not run in the sandbox.

- AC-1: in lucene-rust, `cargo test -p lucene-painless scripted_metric` (the init, combine and reduce runners over the front end's contexts), merged as lucene-rust #599.
- AC-2: in lucene-rust, `cargo test -p lucene-ffi --lib metric_map` (the map script's fold across the fixture's documents and its refusal of another context's script), merged as lucene-rust #600, and `a_scripted_metric_folds_the_documents_and_reduces_the_shards` in crates/node/tests/script_score_painless.rs.
- AC-3: `a_scripted_metric_refuses_what_it_does_not_serve` in crates/node/tests/script_score_painless.rs; the switch off is the existing refusal of an unregistered aggregation.
- AC-4: `a_scripted_metric_folds_the_documents_and_reduces_the_shards` in crates/node/tests/script_score_painless.rs and `a_scripted_metric_reduces_every_shards_value_in_order` in crates/coordinator/tests/it/agg_reduce.rs.
- AC-5: `a_scripted_metric_refuses_what_it_does_not_serve` (compile error, a combine that throws) in crates/node/tests/script_score_painless.rs and `a_scripted_metric_whose_reduce_throws_fails_with_its_name` in crates/coordinator/tests/it/agg_reduce.rs.
