# A write reports its share of its commit window

## Serves

[ADR 0061](../../adr/0061-a-request-reports-its-own-store-requests.md), the follow-up its third decision names: the store requests a commit window makes for all the writes in it (the log blob's PUT, and the epoch's lease write when a window opens one) are shared among the writes that asked to see their cost, by the bytes each put in the window.

## Change

- `oss_blob::cost` gains a splitting scope: `Scope::split` over `(request counters, weight)` pairs and a total weight, `capture_scope()` and `enter_scope()` to carry any scope to another thread. A store request or byte count made in a splitting scope adds to each member its weight's share of the total, in thousandths of a request; a thread in one no longer adds to its own request.
- `Window` (`writer::log::window`) keeps the counters of each writer that joined it with a scope, and the bytes it put in. The thread that writes the window enters the splitting scope for the whole write (epoch, admissions, the blob), so its own request is not charged the window's PUT, and the hedge threads of the PUT enter the same scope.
- A writer that did not ask contributes its bytes to the total and is charged nothing, so the shares of the asking writers add up to at most one PUT.
- Not in this change: reads a write makes for its own mapping update (already counted on its thread), requests the other node makes for a forwarded write.

## Acceptance criteria

- AC-1: in a splitting scope a request of a kind is added to each member in proportion to its weight (two of equal weight 500 and 500 thousandths; weights 1 and 3 give 250 and 750), the bytes likewise, and a thread outside any scope adds to nothing.
- AC-2: `capture_scope` on a thread in a splitting scope and `enter_scope` on another makes that thread's requests count in the same shares; the previous scope is restored when the guard drops, also on unwind.
- AC-3: two writers that ask, with the same bytes, in one window with one PUT each report 500 thousandths of a PUT, and the window's one PUT is not reported twice (the shares add to one request).
- AC-4: a writer that did not ask in the same window is charged nothing and the asking writers' shares are of the whole window's bytes (two asking and one not, equal bytes: each asking writer reports a third of a PUT).
- AC-5: the writer that drives the window is not charged the whole PUT: a single asking writer with a window to itself reports the whole PUT, and with another joiner only its share.
- AC-6: the hedge threads of a PUT count into the same shares (a hedged PUT's two attempts are two requests shared out).
- AC-7: a write that does not ask changes no counter, and no scope remains on a thread after its write returns.
- AC-8: on a booted node with store usage on, a single write that asks, alone in its window, is answered with `put=1` and the bytes it wrote.

## Test plan

`crates/blob/src/cost.rs` unit tests (AC-1, AC-2); `crates/writer/tests/it/node_log_cost_window.rs` (AC-3 to AC-7) over a committer on a counting store, writers joining one window; `crates/node/tests/server/boot_usage.rs` (AC-8). Each seen failing with the behaviour removed.

## S3 request budget

Zero: counter adds for requests already made. A window's store requests are the same with and without a scope.

## Switch

None: a scope exists only for a request that asked, and a write without the header is as it was.
