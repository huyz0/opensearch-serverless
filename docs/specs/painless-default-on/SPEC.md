# Painless native compile: the evidence for turning it on

## Serves

[ADR 0069](../../adr/0069-painless-source-compiles-in-rust-with-java-classes-as-functions.md)'s switch `serverless.scripting.painless.native_compile` (default off) and [principles](../../principles.md#switches-and-defaults): a default is on only when measured with no downside. R14.6.

## Change

None to behaviour. A measurement, `crates/node/tests/painless_default_cost.rs` (`--ignored`), and its record in VERIFIED.md. The default is flipped by a separate PR, and only if every criterion holds.

## Acceptance criteria

- AC-1: a search with a Painless script_score makes the same object-store requests as the same search without a script (by method: GET, PUT, HEAD, LIST), counted by the meter, cold and warm.
- AC-2: over 2,000 warm searches of size 10 on the compat index, the p50 and p99 of a search with a Painless script_score are within 5% of the same search with no script, or the difference is stated as a cost and judged by the owner.
- AC-3: a script that builds a string past 1 MiB fails with a 400 script_exception (the bound of ADR 0069), and the node's resident memory after 200 such requests is within 10% of before.
- AC-4: a script that loops past the loop counter (1,000,000 iterations) fails with a 400 script_exception in under 5 seconds, and the next search is served.
- AC-5: a repeated source is compiled once: the compile share of a search with a script is under 5% of its latency after the first request, held by a cache or by measurement.

## Test plan

One ignored test measures each, prints the numbers, and asserts the criteria. Written first and run before any default changes; a criterion the code cannot meet is recorded as the blocker, not weakened.

## S3 request budget

Unchanged: nothing here adds a request; AC-1 holds it.

## Switch

Unchanged and off. Flip only when AC-1 to AC-5 hold and painless-front-end's AC-4 and AC-5 have evidence.

## Risks

An arena or cache that is not built makes AC-3 and AC-5 unprovable; then the measurement says so.
