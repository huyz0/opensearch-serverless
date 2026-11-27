# Painless compile cache

## Serves

[ADR 0069](../../adr/0069-painless-source-compiles-in-rust-with-java-classes-as-functions.md) ("Cache"), [painless-front-end](../painless-front-end/SPEC.md) and [painless-default-on](../painless-default-on/VERIFIED.md) AC-2 and AC-5: a Painless source was parsed, analysed and checked on every request (twice in this crate's `encode`). R14.6.

## Change

`crates/reader/src/painless_cache.rs`: a bounded cache of what `function_score_painless::encode` computes from the source: the fields a script reads (by source and context) and the engine blob (by context, source, params and the kind of each field it reads). Entries are oldest-out. The cache is process-wide, as the switch is, and is on only when the node installed it: at assembly, with `serverless.scripting.painless.native_compile` on, the node reserves the cache's bytes (4 MiB) from the query-execution pool and holds them. The engine's own per-segment compile of the blob is not cached here.

## Acceptance criteria

- AC-1: the second `encode` of the same source, context, params and mapping returns the same blob without computing again (the cache counts one miss then one hit).
- AC-2: a changed param, a changed kind of a field the script reads, or a changed context gives its own blob; nothing stale is served.
- AC-3: the cache holds at most 256 entries and never more charged bytes than it was given; the oldest entries leave first; a value larger than the whole budget is served and not kept.
- AC-4: the cache's bytes are reserved from the query-execution pool when the node installs it and returned when it is removed; a pool that cannot hold them leaves the cache off and searches are served.
- AC-5: a compile error is not cached and is the same error each time.
- AC-6: with the switch off the node reserves nothing.
- AC-7: the Painless script_score's p50 over a weight is re-measured (`painless_default_cost`) and the change recorded.

## Test plan

Unit tests on the cache and on `encode` (AC-1 to AC-3, AC-5), a node test on the pool (AC-4, AC-6), written first and seen failing (no module). AC-7 is the existing ignored measurement.

## S3 request budget

Unchanged: nothing here reads the store.

## Switch

Part of `serverless.scripting.painless.native_compile`; no setting of its own. The size is a constant (256 entries, 4 MiB) as ADR 0069 states.

## Risks

A key that misses an input serves a stale blob: AC-2 holds the inputs. A single global lock on a hot path: the lock is held for a map lookup and a copy of one blob.
