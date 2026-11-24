# Verified

- AC-1: `a_bucket_members_commit_is_judged_from_its_bucket_entry` (crates/node/tests/dead_logs.rs)
- AC-2: `a_bucket_members_commit_is_judged_from_its_bucket_entry` (one Get, one GetRange, no List)
- AC-3: `a_dead_nodes_log_is_collected_once_a_successor_has_published_all_of_it`
- AC-4: `a_bucket_members_commit_is_asked_of_its_bucket` (the collector asks the bucket; the stub judges coverage)
