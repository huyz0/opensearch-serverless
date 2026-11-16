# oss-testkit

Test-only support shared by every crate's tests: a counting global allocator that records the peak, and helpers to assert peak memory under load. A dev-dependency only; never a normal dependency.

## Owns
Test-only helpers: a counting allocator for peak-memory assertions, temporary directories, and the request-budget table's checker.

## Depends on
Nothing in the workspace.

## Entry points
`src/lib.rs` re-exports them: `counting_allocator()` for a `#[global_allocator]` static (the pinned `cap` crate, so no `unsafe` here), `PeakWindow` for peak, live and allocated bytes since a start point, and `assert_peak_below`. Put each peak test in its own integration-test binary: the counters are process-wide. `TempDir::new(label)` is a fresh directory removed on drop. `budget::check(key, measured, detail)` holds a measurement to its row of [docs/request-budgets.md](../../docs/request-budgets.md), which is compiled in; `OSS_REQUEST_BUDGETS_RECORD=1` prints instead of checking ([spec](../../docs/specs/request-budgets/SPEC.md)).
