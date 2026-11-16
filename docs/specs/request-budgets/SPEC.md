# Request budgets

## Serves

The plan's request-budget table: what the key paths cost in object-store requests and in sequential round trips, pinned in one table so that a change which adds a request fails a test. Follows [settings-registry](../settings-registry/SPEC.md).

Invariants served:
- **S3 requests are the bill:** each user-facing path's request count and round-trip depth is pinned, and a saving cannot be lost silently.
- **A rule that can be a predicate over files is a check:** every row is `enforced` by a test that exists and names it, or `blocked` with its blocker named, and a script checks it.

## Change

Ported from huyz0/OpenSearch at `3920dc36`:
- `serverless/request-budgets.md`: the rules and the table's shape (objective, case, budget, bound, status, gate).
- `serverless/testkit/src/test/java/org/opensearch/serverless/testkit/RequestBudgets.java`: parsing, `check`, `verdict`, record mode.
- `RequestBudgetTableTests.java`: every row enforced or blocked, a malformed row an error, a regression and a saving both failing.
- `RoundTripDepthBlobStore.java`: round-trip depth, the deepest chain of requests a caller waits through.

### Pieces

- **`docs/request-budgets.md`**: the rules and the table. A row's objective is a `lower.case_key`; its budget a number; its bound `exact` (equal, so a saving fails until the table records it) or `max` (a ceiling); its status `enforced` (the gate is a test, `crates/<crate>/tests/<path>.rs::<fn>`) or `blocked` (the gate cell names the blocker). Every Java row for a path not yet ported stays, as a `blocked` row naming the missing port and the Java's number; where the Java's own blocker records why it misses a target, that text stays too.
- **`oss_testkit::budget`**: reads the table compiled into the test binary. `check(key, measured, detail)` fails on a regression and on an exact saving, with what to do, and refuses an unknown or blocked key. With `OSS_REQUEST_BUDGETS_RECORD=1` it prints the measurement and its detail instead of checking.
- **`oss_blob::Rounds<S>`** (behind the `emulator` feature): a `BlobStore` wrapper measuring round-trip depth: a call is one round deeper than the deepest call already answered when it started, so calls sent together share a round. Each call can be held for a fixed delay so calls sent together overlap. It logs each call's kind and key, with a ranged read's range, which a gate passes as the detail, so a regression names the keys it touched.
- **`scripts/check-budgets.py`**, a tree check: every row parses; every enforced row names a test file that is a test target or is declared as a module of one, holding a `#[test]` `fn <name>` that is not `#[ignore]`d and names the row's key in its body; every blocked row names a blocker.
- **Gates**: the reader's cold and warm search, through a node; the descriptor store's create, read, update and delete; index create and delete through `IndexLifecycle`; an alias update; a cluster settings update.

### Not in this change

A simulated clock: no periodic job exists in Rust yet, so the Java's idle-hour rows stay blocked on it and on the writer. A by-caller breakdown from the stack, as the Java's `StoreUsage` does: the detail lists keys instead.

## Acceptance criteria

- AC-1: The table parses. A malformed row is an error naming its line, never a skip: wrong cell count, an objective that is not a key, a budget, bound or status that does not parse, an objective twice. Tests: `the_table_parses_and_keeps_a_blocked_row`, `a_malformed_row_is_an_error_not_a_skip`.
- AC-2: A measurement above the budget fails naming the budget; an exact row's measurement below it fails asking for the lower number; a max row is met from below; an unknown or blocked key is refused; record mode checks nothing. Tests: `a_regression_fails_and_names_the_budget`, `an_exact_saving_fails_until_the_table_records_it`, `check_refuses_an_unknown_or_blocked_objective`, `record_mode_checks_nothing`.
- AC-3: Calls sent together share a round; a call sent after another's answer is the next round; reset starts again at round one; the log names each call's kind and key. Tests: `calls_sent_together_share_a_round`, `a_call_after_an_answer_is_the_next_round`, `the_log_names_each_call_and_its_key`.
- AC-4: The tree check refuses an enforced row whose test file, function or key is missing, whose function is not a test or is ignored, whose key is outside the function's body, or whose file no test target declares; a blocked row with no blocker; and a malformed row. It passes the real table. Evidence: `scripts/selftest-checks.py` plants each.
- AC-5: Each enforced row is checked by the test it names, on the emulator. Tests: `a_cold_search_then_a_warm_one`, `descriptor_operations_cost_what_the_table_says`, `an_index_create_and_delete_cost_what_the_table_says`, `an_alias_update_costs_what_the_table_says`, `a_cluster_settings_update_costs_what_the_table_says`.

## Test plan

`crates/testkit/tests/budget.rs` for the table and the verdicts; `crates/blob/tests/rounds.rs` for depth; the gates beside each crate's suite (`crates/node/tests/it/budgets.rs`, `crates/meta/tests/descriptors/budgets.rs`); the tree check by its planted violations.

## S3 request budget

No request is added. The measurements run on the emulator.

## Switch

None: test and gate code only.

## Risks

- A gate that passes the key with the wrong measurement satisfies the tree check. Review covers that a gate measures what its row says.
- The gates count on the emulator: a register swap is one request there, and on S3 one only when the process just read the generation.
- Round-trip depth is an upper bound: an unrelated call during a measurement counts. Gates measure with nothing else talking to the store.
