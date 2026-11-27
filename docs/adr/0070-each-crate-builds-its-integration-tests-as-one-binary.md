# 0070: Each crate builds its integration tests as one binary

**Status:** Accepted after independent review, 2026-11-27 (a reviewer subagent, given the ADR and principles only, cleared it on condition of the wording above, now applied)

## Context

Cargo builds every file in a crate's `tests/` as its own binary: each is compiled against the library and linked on its own. A mutant's build rebuilds all of them. The nightly sweep's lanes spent "13 s build + 3 s test" a mutant on `oss-coordinator`, 43 test binaries, and its sweep is about 470 runner-minutes a night. After a one-line change to the library, rebuilding the crate's tests took 9.5 to 10.5 s as 43 binaries and 3.9 to 4.3 s as one (4 cores, provisional). The workspace has about 400 test binaries, of which about 35 install a `#[global_allocator]` and several are `harness = false` or measure the process's memory; those stay separate. `docs/ci.md` already names one binary per crate (`tests/it/main.rs`) as the rule, and `oss-node`'s `roles`, `tail` and `server` tests are built that way.

## Decision

A crate's integration tests are modules of one binary, `tests/it/main.rs`, with a doc comment and a `mod` line for each file. A file keeps its inner attributes. Under nextest every test runs as its own process; under `cargo test` they share one, so a test that relies on process-global state (an environment variable, a static, a tracing subscriber) must pass under both runners or stay a binary of its own. A file that installs a `#[global_allocator]`, a `harness = false` target, or a test that reads the process's own memory as its measurement stays a `[[test]]` target of its own named in `Cargo.toml`, outside `tests/it/`: merged, its allocator would apply to every test in the binary. `scripts/check-test-binaries.py` refuses a `#[global_allocator]` inside `tests/it/`. Paths named in specs and `VERIFIED.md` files move with the files.

Crates are merged one at a time, each with the rebuild time measured before and after.

## Consequences

- A mutant's build, and every PR build of the crate, is faster: the measured gain is more than half of the crate's test rebuild.
- A test's nextest id gains the `it` binary (`oss-coordinator::it search::name`); evidence lines name tests by function, so they still resolve.
- Two test files can no longer define the same top-level name in one namespace; a `mod` isolates them.
- The path-based risk router counts a moved file by its new path, so each crate's PR names this ADR.
- Tests that need a process of their own are the rule's exception; the allocator case is checked, and the others are caught by running the suite under nextest and `cargo test`.
- Nothing about S3 request cost, memory, scalability, durability or compatibility changes.
