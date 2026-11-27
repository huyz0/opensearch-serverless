# Painless native compile default: not met

Run on 2026-11-27 in a Linux cloud container, debug build, emulator store, 2 cores (provisional; relative, not absolute). Command: `cargo test -p oss-node --features coordinator,extensions --test painless_default_cost -- --ignored --nocapture`. The test was run before any default changed and fails on AC-2 and AC-3, as recorded; it is `--ignored` for that reason and for its 35 s run.

Result: the default stays off. The criteria that fail are the ones ADR 0069 names as new work (the memory arena, the compile cache).

- AC-1: met. `cargo test -p oss-node --features coordinator,extensions --test painless_default_cost -- --ignored --nocapture` read `[0, 0, 0, 0, 0, 0, 0]` requests for a search with no script and for one with a Painless script_score, warm. The cold equality is held by `a_painless_script_adds_no_store_request` (crates/node/tests/script_score_painless.rs).
- AC-2: NOT MET, read by `cargo test -p oss-node --features coordinator,extensions --test painless_default_cost -- --ignored --nocapture`. Three rounds of 2,000 searches (us): a function_score with a weight p50 618 to 649, p99 1,009 to 1,057; the same with an expression script p50 827 to 921, p99 1,503 to 1,563; with the Painless script p50 2,733 to 2,797, p99 5,178 to 6,344. The Painless script costs 4.2 to 4.4 times the weight at p50 (529% at the worst of p50 and p99) and 3.0 to 3.3 times the expression. Debug build: a release run is NOT-RUN.
- AC-3: NOT MET, read by `cargo test -p oss-node --features coordinator,extensions --test painless_default_cost -- --ignored --nocapture`. A script building a 2 MiB string is served with 200 (max_score 2097152.0); there is no 1 MiB bound. After 200 such requests the process's resident memory read 41,188 KiB against 40,708 KiB before (+1.2%), so nothing accumulated in this run, but no bound stops a script that allocates more.
- AC-4: met, read by `cargo test -p oss-node --features coordinator,extensions --test painless_default_cost -- --ignored --nocapture`. The script while (true) { n++; } answers 400 script_exception ("runtime error") in 45 ms and the next search is 200. The loop counter is the engine's, 1,000,000 by default; no setting changes it.
- AC-5: NOT MET, read by `cargo test -p oss-node --features coordinator,extensions --test painless_default_cost -- --ignored --nocapture`. The first request of a new source took 2,745 us and its repeats p50 3,071 us, so the source is compiled on every request: there is no compile cache. NOT-RUN: the compile share of a search, which needs a release build and a timer inside the engine.

## Blockers to the flip

The default stays off until: a per-invocation memory bound (ADR 0069's arena, 4 MiB and 1 MiB per string or collection); a compile cache; the IR node cap and the call depth of 128 (painless-front-end AC-4); the AC-5 refusals of that spec; and a decision by the owner on a 4x cost over a weight (3x over an expression) or a release-build run that shows less. No default PR was opened.

## Update after the compile cache and the limits

Re-run with painless-compile-cache and painless-limits in place (same command, debug build, three rounds of 2,000 searches, provisional): p50 (us) weight 588 to 613, expression 767 to 826, Painless 1,636 to 1,659; p99 weight 1,014 to 1,139, Painless 2,662 to 2,800. The Painless script_score is 2.7 times the weight at p50 (was 4.3) and 2.0 times the expression; AC-2 is still NOT MET. AC-3 is now met: the 2 MiB string answers 400 script_exception and the resident memory after 200 requests is 40,520 KiB against 40,016 KiB. AC-1 and AC-4 still hold. AC-5: the source is no longer parsed twice a request, but the engine still parses and lowers it per segment (first request 2,994 us, repeats 1,853 us); a cache there is lucene-rust work and is NOT-RUN.

The default stays off: AC-2 and AC-5 do not hold, and the 5% bound is the owner's to waive.
