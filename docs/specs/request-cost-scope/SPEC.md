# Request cost scope

## Serves

[ADR 0061](../../adr/0061-a-request-reports-its-own-store-requests.md), the first piece: the counters of one request's store requests and the thread scope that finds them, in `oss-blob`. The REST edge, the hand-off sites and the response header are the next changes.

## Change

- `oss_blob::cost::RequestCost`: fixed-size atomic counters of one request: milli-requests by `S3Op`, body bytes read and written. Read as whole requests rounded to the nearest, as bytes, and as `cost_milli_units` (the ledger's price: a PUT, COPY or LIST one, a GET or HEAD 0.08, a DELETE none).
- `cost::capture()` returns the thread's scope, if any; `cost::enter(Option<Arc<RequestCost>>)` makes it the thread's scope until the returned guard drops, and the guard restores what it replaced, also when the thread unwinds; `cost::suspend()` is `enter(None)`.
- The ledger's tap (`LedgerTap`) adds each request and byte count to the thread's scope as well, whatever line the ledger charges it to. A thread with no scope pays one thread-local read per request.
- Not in this change: the REST edge that creates a scope, the hand-off sites, the response header, the commit window's null scope.

## Acceptance criteria

- AC-1: with a scope entered, a store call through a `UsageStore` is counted in the scope once per attempt, by kind, with its bytes, and the ledger's lines are as they were.
- AC-2: a call on a thread with no scope changes no scope.
- AC-3: a scope entered on a spawned thread from a captured handle counts that thread's requests in the same counters; `capture()` is none where none is entered.
- AC-4: a guard restores the previous scope when it drops, nested guards restore in order, and a panic inside the guarded work leaves the thread with no scope (the next call on that thread counts nowhere).
- AC-5: `suspend()` makes requests in its guard count nowhere in the scope, and the scope counts again after it drops.
- AC-6: a request kept in thousandths rounds to the nearest whole request (499 to 0, 500 to 1), and `cost_milli_units` prices a PUT 1,000 and a GET 80 thousandths of a PUT.

## Test plan

`crates/blob/src/cost.rs` unit tests: AC-1 to AC-5 through a `UsageStore` over the emulator, AC-6 on the counters.

## S3 request budget

Zero: a counter add per request already made.

## Switch

None here: a scope exists only where a later change creates one for a request that asked.
