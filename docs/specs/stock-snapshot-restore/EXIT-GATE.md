# R14.8 exit gate: stock snapshot interop

The plan's gate for R14.8, clause by clause, with what holds today. A clause is `done` only where a test named here passes; the rest says what is missing. Checked on main after the restore, the recorded fixture and the budget rows merged.

> A snapshot taken by stock OpenSearch 3.10 restores on Rust and answers the same fixtures; a Rust export restores on stock OpenSearch; the requests a restore makes are pinned in the budget table, and the native path's rows are unchanged.

| Clause | Status | Evidence |
|---|---|---|
| A 3.10 snapshot restores on Rust and answers the same fixtures | done, restore only | `a_recorded_stock_snapshot_restores_and_answers_what_the_stock_node_answered`: the repository a stock 3.10 node wrote ([fixture](../../../fixtures/snapshots/stock/README.md)) restores over REST from an `fs` source and the restored index answers the three searches the stock node answered, and a stored document's `_source`. The same recorded repository restores from an emulated `s3` source with no LIST (`a_stock_restore_costs_what_the_budget_says`). |
| The RustFS leg | done, on the hand-built repository | `a_stock_snapshot_restores_from_a_rustfs_source` restores `fixtures/snapshots/stock-repo` from a real RustFS 1.0.6 process with no LIST and every file byte for byte. Run by hand, `RUSTFS_BIN=<path> cargo test -p oss-node --features "reader writer coordinator" --test server -- rustfs`: 1 passed in 9 s, on a binary whose zip SHA-256 matched the recipe (`a4d109a0...`), one run, cloud sandbox, no network latency. Skipped, and says so, without `RUSTFS_BIN`: CI does not run it. Not done: the recorded 3.10 repository through RustFS, and a search over that restore. |
| A Rust export restores on stock OpenSearch | not started | R14.8 is "restore first"; the export has no spec yet. |
| The requests a restore makes are pinned in the budget table | partly done | `snapshot.stock_restore.requests` (18) and `snapshot.stock_restore_cleanup.requests` (22), exact and enforced, measured on the recorded repository. `snapshot.stock_restore.round_trips` is not recorded: the route builds its own source store, so no test can count the source's dependent rounds. `scripts/check-budgets.py` cannot see a row that does not exist. |
| The native path's rows are unchanged | done | `snapshot.restore_two.requests` (10) and `snapshot.restore_cleanup.requests` (22) pass unchanged (`a_restore_costs_what_the_budget_says`, `a_restore_that_fails_part_way_removes_what_it_created`). A restore of a snapshot the native register does not hold costs one request more, the repository record, and the switch-off test pins that miss at 4. |

## What closes the gate

1. Export to a stock repository, with its own spec.
2. The round-trips row: a way to count the source's dependent rounds (the route taking its source store from the service, so a test can wrap it), then the row.
3. The recorded repository through RustFS, searched.
