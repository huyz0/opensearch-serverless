# AGENTS.md

Orientation for AI agents and for people joining. A router plus invariants: it does not repeat [docs/](docs/) or [.agents/skills/](.agents/skills/). On conflict, the doc or skill wins for its topic; fix the drift.

## What this is

A Rust serverless OpenSearch: stateless reader, writer and coordinator nodes over object storage, built as a strangler beside the Java serverless shell (huyz0/OpenSearch, `feature/serverlessshell`) and sharing its blob formats. Engine: lucene-rust. Runtime: hsearch. Plan: [docs/roadmap.md](docs/roadmap.md). Map: [docs/architecture.md](docs/architecture.md). Rules: [docs/principles.md](docs/principles.md).

## Invariants (don't break)

1. **Every S3 call goes through `oss-blob` and is metered.** LIST is priced like a PUT: prefer a derived key, then a GET; every LIST carries `// LIST:`. Never add a PUT to save LISTs. Idle indices make zero periodic requests.
2. **One write log per node, never per shard. One immutable blob per segment.**
3. **Downward-only crate dependencies;** reader and writer never depend on each other.
4. **Small modules:** under 500 lines ideally, 1,000 at most; one concern each; private by default; every public item documented.
5. **No `unsafe`** anywhere (`unsafe_code = "forbid"`).
6. **Build behind switches;** on by default only when measured with no downside (write p99 within 5%, no extra S3 requests, no search regression, crash tests green). No downside, no switch.
7. **Tests use emulators only:** the in-process S3 emulator or RustFS. Nothing needs Docker.
8. **≥95% line coverage;** never lower a threshold or delete a test to pass a check.
9. **Never claim a test passed, a gate ran or a number was measured without having done it.** Emulator numbers are provisional.
10. **A gate nobody has seen fail is not trusted.** A new check is run against the defect it targets first, and its blind spots go in [docs/mechanical-gates.md](docs/mechanical-gates.md).
11. **Comments say what the code does;** no milestone, phase or task ids.

## Commands

```bash
scripts/bootstrap.sh              # fresh clone to green gate (Linux, macOS, Windows Git Bash)
scripts/gate.sh                   # the whole gate; run before calling work done
scripts/coverage.sh --summary-only   # per-file coverage
cargo test -p oss-blob            # one crate
cargo run -p oss-node --features writer,coordinator
```

The gate is defined once, in [scripts/gate.sh](scripts/gate.sh); `.githooks/pre-commit` and CI run it. Install hooks with `scripts/setup-hooks.sh` (bootstrap does it).

## How work flows

1. **Spec first** for a feature: `docs/specs/<feature>/SPEC.md` with checkable `AC-<n>` criteria ([docs/specs/](docs/specs/README.md)). `VERIFIED.md` names the evidence when done.
2. **One branch and PR per change.** Commit subject `type(scope): lowercase description` (`feat|fix|docs|test|chore|refactor|perf|build|ci`); feat, fix and perf commits carry `Spec: docs/specs/<feature>/SPEC.md` or `Spec: none (<reason>)`. **No AI or Claude attribution** anywhere: no Co-Authored-By, session links or "generated with" footers in commits or PR bodies.
3. **PR body follows the template:** Intent, Alternatives rejected, Invariants, Evidence, Risk tier, Where to look.
4. **The thread that wrote a PR merges it** (squash) once `ci-ok` and `pr-ok` are green, after at most two review rounds (`/quality-review`).
5. **High-risk** (formats, durability or fencing, S3 request pattern, crate dependencies, public API, defaults, security; paths in [scripts/risk-paths.txt](scripts/risk-paths.txt)): link or add an ADR; CI labels it `needs-huy`. Merge it too, then keep the thread open and flag the change to the owner for review. Only the owner accepts ADRs. A PR failing only on a known flaky test merges; the flake gets its own PR.
6. Keep harness work (scripts, CI, gates) to about 20% of changes; ship product.

## Where to look

| Task | Read |
|---|---|
| Where code belongs, adding a crate | `architecture` skill, [docs/how-to/add-a-crate.md](docs/how-to/add-a-crate.md) |
| Anything touching S3 | `s3-cost` skill, [docs/how-to/add-an-s3-call.md](docs/how-to/add-an-s3-call.md) |
| A setting or a feature switch | [docs/how-to/add-a-setting.md](docs/how-to/add-a-setting.md) |
| REST, extensions | [docs/how-to/](docs/how-to/README.md) |
| Writing tests | `test-coverage` skill |
| Optimising | `rust-performance` skill |
| Finishing a change | `code-review` skill, then `/quality-review` |
| Committing | `git-workflow` skill |
| A decision with trade-offs | add an ADR in [docs/adr/](docs/adr/README.md) |
| What must never break, and its tests | [docs/invariants.md](docs/invariants.md) |
| Working inside one crate | that crate's `AGENTS.md` and `README.md` |
| Editing skills or this file | `manage-skills` skill |
| Replies and reports | `brevity` skill |
| Why a practice is or is not here | [docs/harness-review.md](docs/harness-review.md) |
