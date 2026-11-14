# What this repo takes from lucene-rust, hsearch and pstore

A review of each project's engineering practices and AI harness (agent instructions, skills, hooks, gates, eval tooling), with a verdict for this repo. **Adopted** items are in the tree. **Proposed** items wait for the owner's go-ahead. **Rejected** items say why. A separate set of ten practices for keeping AI-built code understandable was approved on 2026-11-14 and is applied; it is listed last.

## From lucene-rust (applied)

| Item | Verdict | Why |
|---|---|---|
| `AGENTS.md` as a short router plus invariants, skills hold the procedures | Adopted | Keeps always-loaded context small; a human reads the same file |
| Exact toolchain pin in `rust-toolchain.toml` | Adopted (1.97.1) | Same pin as hsearch and lucene-rust, so git dependencies build together |
| One `scripts/gate.sh` that the hook, CI and docs all call | Adopted | The definition cannot drift |
| `commit-msg` hook: conventional subject | Adopted, without the `Co-Authored-By` rule | The owner wants no AI attribution in commits; a `Spec:` trailer is required instead |
| `pre-commit` hook running the gate | Adopted, native | No Docker in the cloud sandboxes (lucene-rust's capped container is rejected below) |
| `clippy::arithmetic_side_effects` with `// ARITH:` proofs | Adopted, workspace-wide | Starting from an empty tree there is no backlog of 2,000 sites, so it can be on everywhere |
| ≥95% line coverage in the gate | Adopted | Same bar as lucene-rust and pstore; hsearch's 90% is lower |
| `check-dev-notes`: no milestone or task ids in comments | Adopted, adapted patterns | Also one of the Java shell's principles |
| "A gate nobody has seen fail is not trusted"; blind spots written down | Adopted | Each check here was run against a planted defect ([mechanical-gates.md](mechanical-gates.md)) |
| rustdoc link lints in the gate | Adopted | Not caught by fmt, clippy or test |
| nextest config with a 10-minute hang limit | Adopted | Config only; CI uses it once the suite is large enough to need it |
| `x86-64-v3` target CPU on Linux | Adopted | Deployment target; measured gains in lucene-rust |
| Skills: brevity, code-review, git-workflow, manage-skills, rust-performance, test-coverage, architecture | Adopted, rewritten for this repo | The port-specific parts (Java fixtures, parity ledger) do not apply |
| `quality-reviewer` subagent and `/quality-review` command | Adopted, rubric rewritten | Judgement the gates cannot make: cost per operation, switch discipline, layering intent |
| Capped Docker container for every build (`docker-test.sh`), preflight disk checks | Rejected | Cloud sandboxes have no Docker; this workspace is small |
| Port from Java, traceable to its source (lucene-rust's inventory idea) | Adopted after the mission was set | `docs/port-map.tsv`, `scripts/check-port-map.py`, [porting.md](porting.md) |
| Port, benchmark, optimise; differential tests against Java-written bytes | Adopted | Blob fixtures from the Java shell, REST fixtures from OpenSearch ([compatibility.md](compatibility.md)) |
| lucene-rust's parity ledger and per-class inventory gate | Rejected | One port-map row per Rust file is enough at this size |
| `ffi-safety` skill | Deferred | No FFI yet; arrives with the extension loader |
| "Work directly on main" | Rejected | The Java shell's rule wins: a PR per change, reviewed and merged by its thread |

Since [ADR 0002](adr/0002-runtime-dependencies.md), hsearch and pstore are references only: ideas below are re-implemented here, never imported.

## From hsearch (proposed)

| Item | Verdict | Why |
|---|---|---|
| RustFS digest pin and `scripts/rustfs.sh` | Adapt | Keep RustFS as an optional wire-level backend, run as a plain binary, not a container |
| Committed / applied / searchable / archived as distinct coordinates in every receipt | Adopt | Makes freshness explicit in responses; cheap to design in now |
| Resource standard: every buffer, queue, cache and retry loop bounded before allocation, with a limit-and-one test | Adopt | Fits the scale target; goes into the code-review skill |
| Requests scale with nodes and bytes, never documents, indices or time | Already adopted | Same as the Java principle |
| Extension ABI v1 (C and WASM) | Adapt | Design this repo's ABI from it; no dependency |
| Review cap: at most two planned rounds, three hard, then split | Adopted | In the code-review skill |
| `tasks.json` ledger, `plan.py next`, task packets, review packets with JSON schemas, worktree and resource-lease registry | Reject | Heavy process built for many parallel agents on one machine; Claude project threads and PRs already give one branch per task. hsearch itself has frozen this harness and caps harness work at 20% |
| "Harness frozen; add a check only for a defect that has shipped" | Reject as stated | Conflicts with lucene-rust's practice and the owner's ask for automated checks of the principles. Keep its spirit: no gate without a named defect class |
| 90% per-crate coverage | Reject | Lower than the 95% lucene-rust and pstore hold |

## From pstore (proposed)

| Item | Verdict | Why |
|---|---|---|
| `gate-design` skill: the enforcement ladder (unrepresentable, derived, deterministic gate, generated, agent review, prompt last) | Adopt | The best single rule for deciding how to enforce a new principle |
| `tdd` skill: write the test first and watch it fail; assert properties, not implementation | Adopt | Matches the "seen it fail" invariant |
| Review skill with a round budget (2 nominal, 4 hard), minors never block, reviewer not given the author's reasoning | Adopt the rules; reject `review.sh` packets for now | The rules stop review loops; the packet tooling is more than this repo needs yet |
| Cost and depth as named tests: "a write issues exactly 1 PUT", "no LIST on hot paths", sequential depth ≤ N | Adopt | Turns the cost principles into failing tests |
| Mutation testing (`cargo mutants`), incremental on the diff | Adopted | `scripts/mutants.sh`, CI on pull requests |
| `cargo deny`: licences, advisories, and a ban on S3 clients outside the blob crate | Adopt | Stronger than the grep in `check-s3-access.py` (it sees transitive dependencies) |
| `check-poison`: recover poisoned locks, never drop the `Result` | Adopt when the first lock lands | A real shipped defect in pstore |
| `scripts/lib/py.sh`: probe for a Python that actually runs | Adopted already | Needed on Windows, where `python3` is the Store stub |
| `check-portable`: scripts run on Linux, macOS and Windows; gates run on all three in CI | Adapt | Linux in CI (cheap), Windows locally; skip macOS runners for a private-budget repo |
| CI path filter: docs-only changes skip the Rust jobs | Adopt when CI time matters | |
| "Never claim a test passed without running it"; never weaken a threshold | Adopted | In principles and AGENTS.md |
| A feature with a measured downside is a setting, cluster default, per-index override | Already adopted | Same as the Java switches rule |
| SPEC.md / VERIFIED.md per feature with an evidence check | Adopted | `docs/specs/`, `scripts/check-verified.py` |
| SLO ledger with `enforced` or `blocked`, generated AGENTS.md index | Reject for now | Revisit when there are service objectives to state |
| Research corpus routing (`research` skill, INDEX.md) | Reject | This repo has no corpus; the assessment doc is linked instead |

## Where the sources conflict

- **Branching:** lucene-rust commits to main; hsearch and the Java shell use one branch and PR per task. This repo uses PRs.
- **Adding gates:** lucene-rust and pstore add a gate per defect class; hsearch froze its harness. This repo adds checks for its stated principles and requires each to be seen failing.
- **Coverage:** 95% (lucene-rust, pstore) vs 90% (hsearch). This repo uses 95%.
- **Docker:** lucene-rust runs everything in a capped container; this repo must run without Docker.
- **Unsafe:** lucene-rust allows it in three crates, pstore in one, hsearch in one; this repo forbids it everywhere until an extension loader needs an exception.

## Approved 2026-11-14: keeping AI-built code understandable (applied)

| Practice | Where |
|---|---|
| architecture.md under 1,500 words, crate graph generated from `cargo metadata`, drift fails CI | `scripts/gen-crate-map.py --check` |
| SPEC.md per feature with checkable criteria, VERIFIED.md with evidence per criterion | `docs/specs/`, `scripts/check-verified.py` |
| PR template (intent, alternatives rejected, invariants, evidence, risk tier, where to look), checked in CI; squash merge keeps it | `.github/pull_request_template.md`, `scripts/check-pr.py` |
| Fitness functions with planted-violation self-tests and a ratchet | `scripts/selftest-checks.py`, `scripts/size-baseline.txt`, `scripts/check-unsafe.py` and the other checks |
| ADR required for formats, crate dependencies, public API, S3 request pattern, defaults; only the owner accepts | `scripts/check-pr.py`, `docs/adr/README.md` |
| docs/invariants.md with each invariant linked to its tests, links checked | `scripts/check-invariants.py` |
| proptest, incremental cargo-mutants, coverage floor; differential tests against Java and lucene-rust fixtures arrive with the reader | `crates/blob/src/meter.rs`, `scripts/mutants.sh` |
| High-risk paths get the `needs-huy` label. Changed by the owner on 2026-11-14: it flags the change for review after merge and does not block it | `scripts/risk-paths.txt`, `.github/workflows/pr.yml` |
| `Spec:` commit trailer, checked in the hook and CI (no session-link trailer and no AI attribution, by the owner's choice) | `scripts/check-commit-msg.sh` |
| Short root AGENTS.md plus one per crate; reviews bounded at two rounds; harness work kept near 20% | `AGENTS.md`, `crates/*/AGENTS.md` |

Later: code tours, simulation and fuzz tests, Kani on CAS and fencing, a monthly docs-only agent quiz, newcomer metrics.
