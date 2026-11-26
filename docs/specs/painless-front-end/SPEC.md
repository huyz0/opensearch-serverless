# Painless source compiled in Rust

## Serves

[ADR 0069](../../adr/0069-painless-source-compiles-in-rust-with-java-classes-as-functions.md); R14.6's remaining gap (a node cannot compile Painless source).

## Change

A Rust front end in lucene-rust (`lucene-painless/src/front/`) turns Painless source into the IR the runtime already runs, with Java library classes served as functions over plain values (dates are epoch `long`s). The node uses it for `script_score` (and later scripted sort, `script_fields`, the ingest `script` processor) behind `serverless.scripting.painless.native_compile` (off). The runtime gains a `Date` value kind and an allocation arena (ADR 0069). Unchanged: `check`'s rules, the expression language, every Java-written IR blob.

## Acceptance criteria

- AC-1: every one of the 4,964 corpus sources is parsed or refused with a position and a reason, and no source Java did not compile (status 2) is accepted.
- AC-2: for every corpus script the front end accepts that uses no replaced object, the native result or exception equals Java's recorded one, the count of differing results is 0, and `check` still passes only scripts that match (`unsound = 0`).
- AC-2b: `script_score`, filter and sort scripts of `fixtures/data/painless-score` give Java's scores or the same exception through the front end, and a `doc` date, `_score` and `params` read as stock reads them.
- AC-3: the accepted share of the corpus has a floor in the test that only rises.
- AC-4: a script past any bound (source 65,535 bytes, parse depth 100, 20,000 IR nodes, 1,000,000 loop statements, call depth 128, 4 MiB per invocation, 1 MiB per string or collection) fails with `script_exception`, and the arena's charged bytes after 1,000 documents equal those after 1 (the arena counter and the allocator's peak are both asserted).
- AC-5: no library object exists in compiled source (`long x = doc['d'].value` is refused as stock refuses it; `Math.random` is refused in a score context): a script naming `ZonedDateTime`, `Optional`, a stream or `Class` is refused at compile time with its offset; each date function equals Java's value for the same epoch on a recorded table.
- AC-6: with the setting off, a Painless script is refused as today, response identical.
- AC-7: a search with a Painless script issues the same object-store requests as without one, asserted by the meter.

## Test plan

Slice 1 writes AC-1's test first (red: no parser). AC-2's differential test reuses `tests/corpus.rs`' reader. AC-4 has one test per bound. AC-5's table is recorded from stock OpenSearch, not computed by the code under test.

## S3 request budget

Unchanged: compilation is in memory; the compiled script is cached by (source, context).

## Switch

`serverless.scripting.painless.native_compile`, default off. Flips on when AC-1 to AC-7 pass and the corpus floor is recorded.

## Risks

A typing rule that differs from stock where the corpus has no script (revealed by the unsound count and e2e shapes); `def` semantics at call sites (the `check` pass stays the guard); the function table drifting from stock's allowlist (generated check against `spi/*.txt`).
