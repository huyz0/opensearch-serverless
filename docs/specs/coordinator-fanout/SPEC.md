# The coordinator asks its shards concurrently and reports who answered

## Serves

Parity plan R7.2, search fan-out and reduce, third piece: the shell's `Fanout` (a bounded sliding window of per-shard work in which the caller is one worker) and the coverage the shell's `SearchFanout` reports over it (answered, failed, skipped, and the first failure when nothing answered). The per-shard call itself, placement and the activation budget are the node assembly's.

## Change

`oss-coordinator` gains `fanout`: `run`, which runs one function over a slice of items, at most `concurrency` at a time, and gives each item's outcome in item order; and `gather`, which splits the outcomes into the answers, the failures and the `Coverage` (`total`, `successful`, `skipped`, `failed`), or gives the first failure when nobody answered.

## Acceptance criteria

- AC-1: `run` gives one outcome per item, in item order, whichever finished first.
- AC-2: never more than `concurrency` calls are in flight; a `concurrency` of zero runs one at a time; with one, every call runs on the caller's own thread.
- AC-3: a call that returns an error fails only its own slot; every other item still runs.
- AC-4: a call that panics fails only its own slot, as `TaskError::Panicked`; every other item still runs.
- AC-5: `gather` returns the answers in item order, the failures with their item index, and `Coverage` with `total` the shards asked of plus those `skipped` before, `successful` the answers plus `skipped`, `failed` the failures.
- AC-6: `gather` over outcomes that include at least one item and no answer returns the failure of the lowest item index; over no outcomes it returns an empty answer.

## Test plan

`crates/coordinator/tests/fanout.rs`: one test per criterion; AC-2 counts calls in flight with an atomic peak, and the thread each call ran on.

## S3 request budget

Zero: the fan-out calls what the caller hands it; the caller's calls are metered where they are made.

## Bounds

At most `concurrency` threads, the caller's among them; one outcome per item. Threads are scoped, so none outlives the call.

## Switch

None: the shell's fan-out has no setting of its own; the concurrency is the caller's (the shell's `search.fanout_concurrency`, default 8).

## Risks

There is no deadline here; each call carries its own (the transport's), where the shell's five-minute ceiling was a backstop for calls without one.
