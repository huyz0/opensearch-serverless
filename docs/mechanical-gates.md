# Mechanical gates

Every check, what it enforces, the planted violation that proves it can fail, and what it cannot catch. A check nobody has seen fail is not trusted: `scripts/selftest-checks.py` (first step after clippy in `scripts/gate.sh`) builds a clean tree and a tree with each planted violation, and fails if any check passes on its violation. Add a case there when you add a check. The self-test earned its place on day one: it caught `check-unsafe.py` failing on the clean `unsafe_code = "forbid"` line (a regex that backtracked).

| Check | Enforces | Planted violation | Cannot catch |
|---|---|---|---|
| `cargo fmt --check` | formatting | | |
| `cargo clippy -D warnings` (all features, then default) | the lint set, `unsafe_code = "forbid"`, `missing_docs = "deny"`, `arithmetic_side_effects` | an undocumented `pub fn` (run by hand) | a doc comment that says nothing |
| `check-layering.py` | downward-only crate dependencies (so no cycles), siblings independent, every crate placed | reader depending on writer; an unplaced crate | dependencies through non-workspace crates |
| `check-crate-layout.py` | README with Owns / Depends on / Entry points, AGENTS.md, `src/lib.rs`, workspace lints | README without Entry points; no AGENTS.md; lints opted out | a section that is wrong |
| `check-module-size.py` | 1,000-line limit; over 500 only on the shrinking `scripts/size-baseline.txt` | 1,001 lines; 501 lines unlisted; baselined file grew | a short file mixing concerns |
| `check-unsafe.py` | nothing relaxes `unsafe_code` | `#![allow(unsafe_code)]`; `unsafe_code = "warn"` | unsafe code in dependencies |
| `clippy.toml` disallowed methods | no whole-buffer reads (`read_to_end`, `read_to_string`, `fs::read`), no unbounded `mpsc::channel` | `read_to_end` in reader (run by hand) | the same done by a dependency |
| `check-alloc.py` | every allocation in blob, reader, writer, coordinator carries `// ALLOC:`; no unbounded channels | bare `Vec::new()`; `mpsc::channel()` with a comment | allocation inside called functions; false bounds (peak-memory tests do) |
| `check-s3-access.py` | S3 clients only in `oss-blob`; every LIST has `// LIST:` | `aws-sdk-s3` in reader; bare `list_objects_v2(` | LISTs under other names; transitive clients |
| `check-test-endpoints.py` | no real AWS endpoints in code or config | an `amazonaws.com` URL | endpoints from the environment |
| `check-arith-allows.py` | every arithmetic allow carries `// ARITH:` | a bare allow | a wrong proof |
| `check-dev-notes.py` | no milestone, phase or task ids in comments | `// added in M3` | process notes in plain words |
| `check-port-map.py` | every crate has a `port-map.tsv`; every Rust source file has a row in it; `port` rows name Java, `new` rows a reason | a file without a row; a port row without Java | a Java path that does not exist at its commit |
| `check-invariants.py` | every invariant in `docs/invariants.md` names an existing test or check, or says `NOT-YET:` | a missing test name; no evidence | evidence that does not test the invariant |
| `check-verified.py` | every criterion of a finished spec has resolvable evidence | a criterion without a line; a missing test name | whether the evidence is true |
| `gen-crate-map.py --check` | the architecture crate graph matches `cargo metadata`; architecture.md under 1,500 words | (run by hand: edit the graph) | stale prose elsewhere in the doc |
| `check-links.py` | relative Markdown links resolve | a link to a missing file | anchors, external URLs |
| `cargo doc -D warnings` | rustdoc links resolve | | links in plain backticks |
| `coverage.sh --fail-under-lines 95` (local gate, nightly) | line coverage over all features, `main.rs` excluded | | tests that assert nothing (mutants does) |
| `mutants.sh` (PR: a budgeted shard of the diff, `mutants-plan.py`; local: the whole diff; nightly: all) | no surviving mutant | | changes to tests alone |
| `check-commit-msg.sh` (hook and `ci` workflow) | subject format; `Spec:` on feat, fix, perf | | a Spec that does not match the change |
| `check-pr.py` (`ci` workflow) | template sections; high-risk paths need an ADR and a `high` tier | missing section; risky path without an ADR | public API changes outside risky paths |
| `ci-changes.py --self-test` (CI tree checks) | path classification: docs-only skips Rust, crates pull in dependents, root files run all | | a crate reading a file outside its directory |
