# Verified: extension refusal metric

- AC-1: `each_refused_open_counts_once_by_why_and_a_good_open_counts_nothing` (crates/node/tests/extension_modules/refusals.rs): a good pinned open and an index with no pins move neither total, a missing module moves unavailable by one, and an open with the pool left one MiB moves busy by one. Red first: the test did not compile before the refusals module existed. Passed on its first run after.
- AC-2: `the_extension_refusals_are_served_by_why` (crates/node/tests/metrics_route.rs): both reason samples are in the body with HELP and TYPE (the file's grammar parser checks every family), and after one unavailable and two busy records they read the earlier values plus one and plus two. Red first: no such sample existed.
- AC-3: `a_scrape_makes_no_blob_request` (same file) runs unchanged with the family present and passes.
- NOT-RUN: the counts on a booted node's reader and writer paths through HTTP (the counter sits in the module source both use, and the writer and reader tests of extension-analyzer AC-14 exercise those opens, but none of them reads the metric).
