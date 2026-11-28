# Painless execution limits

## Serves

[ADR 0069](../../adr/0069-painless-source-compiles-in-rust-with-java-classes-as-functions.md) "Bounds" and [painless-front-end](../painless-front-end/SPEC.md) AC-4; [painless-default-on](../painless-default-on/VERIFIED.md) AC-3 (memory). R14.6.

## Change

lucene-rust's `lucene_painless::arena` charges every string, array and list a script builds to a per-thread invocation counter and fails the run (a `PainlessError`) at its next loop iteration or its end when it passed a limit; `front::source::compile` refuses a script over the IR node cap. The limits are unlimited until set. A node with `serverless.scripting.painless.native_compile` on sets them when it builds its search service: 4 MiB an invocation, 1 MiB a string or collection, 20,000 IR nodes. With the switch off it sets them back to unlimited. The engine is pinned to the lucene-rust commit that holds them (huyz0/lucene-rust#616, `dbb34e98`).

## Acceptance criteria

- AC-1: with the switch on, a script that builds a 2 MiB string fails with a 400 script_exception, and a script under the limit is served.
- AC-2: many strings each under 1 MiB that add up past 4 MiB in one invocation fail the same way.
- AC-3: a script over 20,000 IR nodes is refused with a 400 and the count.
- AC-4: after a failure the next search is served and the process's resident memory after 200 failing requests is within 10% of before.
- AC-5: with the switch off nothing is limited and no script runs (Painless is refused as before).

## Test plan

`crates/node/tests/painless_limits.rs` written first and seen failing (a 2 MiB string is served with 200). The engine's own tests are in lucene-rust (`tests/arena.rs`).

## S3 request budget

Unchanged.

## Switch

Part of `serverless.scripting.painless.native_compile`; the limits are constants of ADR 0069, not settings.

## Risks

The charge is total allocated in the invocation, not live bytes: a script that makes many short-lived strings can fail where Painless would not. Not charged: growth of a list or map by add and put, a StringBuilder's appends. The peak is the limit plus one operation, and the 4 MiB is not reserved from the node pool.
