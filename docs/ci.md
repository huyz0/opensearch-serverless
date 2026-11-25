# CI: fast and free

AI writes code faster than a shared, free CI can build it, so CI is designed around scarcity. Research basis: [Fast, low-cost CI for an AI-built public Rust repo](https://claude.ai/artifact/1LrX2LhkZaeKeFBmFxhdX8) (2026-11-14).

## The constraints

- Standard GitHub runners are free and unlimited in minutes on a public repo, but the owner's account gets **20 concurrent jobs shared across all of their repos**. lucene-rust has measured 50 to 80 minute queue waits. Queue time, not minutes, is the scarce resource.
- `ubuntu-latest` is 4 CPU and 16 GB. The Actions cache is 10 GB per repo, evicted after 7 days unused.
- No self-hosted runners: on a public repo any fork's PR could run code on them.

## Principles, in priority order

1. **The local gate is the real CI.** `scripts/gate.sh` runs on every `git push` (pre-push hook) and before calling work done. GitHub confirms; it does not discover.
2. **One job per PR push, the only required check (`ci`).** It skips inside its steps instead of fanning out into jobs. No matrix on PRs.
3. **Path-aware.** `scripts/ci-changes.py` decides: docs, scripts and skills run only the tree checks (seconds, no build); a Rust change builds and tests the changed crates and their dependents; `Cargo.lock`, the toolchain, the root manifest, `.cargo/` or CI files run everything. An unrecognised path runs everything.
4. **Cancel superseded PR runs; skip drafts.** Never cancel runs on main or the nightly. Description and label edits run in their own concurrency group and check only the PR text, so an edit never cancels the run checking the code.
5. **The PR job stays under 10 minutes.** Mutation testing on a PR is budgeted: at most 40 of the diff's mutants, one shard chosen from the head commit (`scripts/mutants-plan.py`), within 7 minutes, in place on the tree the tests step built (`MUTANTS_IN_PLACE`). Test processes, and so each mutant's, run under `scripts/memcap-runner.sh` (a 2 GiB address-space cap, `MEMCAP_KIB`): a mutated loop bound that pushes into a Vec until memory is gone failed its own test instead of killing the runner ("The runner has received a shutdown signal"). A missed mutant fails; a mutant that only times out counts as caught (the tests did not pass) and running out of time only warns. A large change runs `scripts/mutants.sh` (every mutant in the diff) locally and names the result in the PR's Evidence; the nightly sweep tests everything. A test that takes over a second goes to the slow tier: mark it `#[ignore = "slow"]` and it runs nightly.
6. **Nightly on main, at most 4 parallel jobs:** coverage at 95%, one 56th-slice of the mutation sweep per lane, eight lanes a night run four at a time, so every mutant is tested every seven nights (the workspace has about 12,900; a lane tests about 230 in its 2 h, so a 28th-slice left its second half untested: nightly run 45158174966 got through 190 to 270 of about 460 per lane), the slow tier, arm64 clippy, and 10 minutes per chunkjson fuzz target. Mutants run their tests under nextest where it is installed (`scripts/mutants.sh`; the nightly lanes install it): on `oss-node` a mutant's test step took 28 s instead of 46 s, with the same 4 missed and 5 caught on the same nine mutants; doctests are left to the PR's tests step. Later: the Java differential suites. A red job opens or updates an issue titled `nightly: <job>`.
7. **Cache written only by main and nightly** (`save-if` on main), so PR branches cannot evict it. `Cargo.lock` is committed and every build uses `--locked`; lucene-rust is pinned by commit.
8. **Lean builds:** `CARGO_INCREMENTAL=0`, no debug info in tests, `--locked`, only the features a step needs, and one integration-test binary per crate (`tests/it/main.rs` with modules), because each extra binary is a separate link.
9. **Every job has `timeout-minutes`;** nextest kills a hung test after 10 minutes.
10. **If 20 slots are not enough:** move the repo to a free GitHub organisation for its own pool (unverified), then a third-party free tier. Never self-hosted.
11. **Measure:** `ci-report.yml` writes a weekly summary of queue wait, minutes per run and the slowest jobs (`scripts/ci-report.py`).

## What runs where

| Where | When | What |
|---|---|---|
| pre-commit hook | every commit | fmt, the tree checks, commit message (seconds) |
| pre-push hook | every push | the whole gate: clippy, tests, coverage, docs |
| `ci.yml` | each PR push, main | PR text and risk, commit messages, tree checks; for Rust changes fmt, clippy, docs, tests of affected crates, mutants on changed lines |
| `nightly.yml` | 03:17 UTC | coverage, mutants (8 lanes, four at a time, one 56th each), slow tests, arm64 clippy, chunkjson fuzzing |
| `ci-report.yml` | Mondays | cost and queue report |

## Enforced by

- `scripts/ci-changes.py --self-test` (in the tree-checks step) pins the path classification.
- The workflows themselves; changing one runs everything, so a broken workflow shows at once.
- Not mechanical: keeping the PR job under 10 minutes. Watch the weekly report.
