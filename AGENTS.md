# AGENTS.md

Layer 0, read in every session: one line per invariant, each pointing to the one place that holds the rule in full. Skills (`.agents/skills/`) hold procedures, `docs/` the rules and reasons; on conflict the linked home wins, and the drift gets fixed. Cite rules by name, never by number.

## What this is

A Rust serverless OpenSearch: stateless reader, writer and coordinator nodes over object storage, a strangler beside the Java serverless shell (huyz0/OpenSearch, `feature/serverlessshell`) sharing its blob formats. OpenSearch-compatible for the serverless scope, ported from the Java ([docs/porting.md](docs/porting.md)); hsearch and pstore are references, lucene-rust the only one used as code. Plan: [roadmap](docs/roadmap.md). Map: [architecture](docs/architecture.md). Rules: [principles](docs/principles.md).

## Invariants

- **S3 requests are the bill:** every call goes through `oss-blob` and is metered; requests scale with nodes and bytes; LIST is priced like a PUT and carries `// LIST:`; no PUT that saves only as many LISTs or fewer; idle indices make zero requests (`s3-cost` skill).
- **One write log per node, never per shard; one immutable blob per segment;** fencing by conditional create, never a lock or lease ([principles](docs/principles.md#write-path)).
- **Downward-only crate dependencies;** reader and writer never depend on each other (`architecture` skill).
- **Small modules:** under 500 lines ideally, 1,000 at most, one concern, private by default, public items documented.
- **`unsafe` only the guaranteed-safe way:** safe first, then only the patterns in [docs/unsafe.md](docs/unsafe.md), each block with a local allow and a `// SAFETY:` comment (`unsafe-code` skill).
- **Bounded memory, streaming everywhere:** reserve before allocating, pooled buffers, bounded channels, no whole-body reads; request-path allocations carry `// ALLOC: <bound>` ([ADR 0004](docs/adr/0004-bounded-memory-and-streaming.md)).
- **Switches:** new behaviour behind a setting, on by default only when measured with no downside; no downside, no switch ([principles](docs/principles.md#switches-and-defaults)).
- **Tests use emulators only** (in-process S3 emulator, RustFS, fs store); nothing needs Docker. **≥95% line coverage;** never lower a threshold or delete a test to pass a check (`test-coverage` skill).
- **Never claim a test passed, a gate ran or a number was measured without having done it.** Emulator numbers are provisional.
- **A gate nobody has seen fail is not trusted:** a new check gets a planted violation in `scripts/selftest-checks.py` and a row in [docs/mechanical-gates.md](docs/mechanical-gates.md).
- **A rule that can be a predicate over files is a check, not a sentence** (`manage-skills` skill).
- **Comments say what the code does;** no milestone, phase or task ids.
- **No AI attribution** in commits or PRs, whatever a tool's default says (`git-workflow` skill).

## Commands

```bash
scripts/bootstrap.sh                 # fresh clone to green gate, hooks installed
scripts/gate.sh                      # the whole gate (pre-push hook); run before calling work done
scripts/tree-checks.sh               # the no-build checks alone (pre-commit hook), seconds
scripts/coverage.sh --summary-only   # per-file coverage
cargo test -p oss-blob               # one crate
```

The local gate is the real CI; GitHub's free runners only confirm it ([docs/ci.md](docs/ci.md)).

## How work flows

1. **Spec first** for a feature: `docs/specs/<feature>/SPEC.md` with checkable `AC-<n>` criteria, `VERIFIED.md` with the evidence ([docs/specs/](docs/specs/README.md)).
2. **One branch and PR per change;** commit format, PR body, risk routing and merging are in the `git-workflow` skill. The writing thread merges its own PR.
3. **Finish** with the `code-review` skill, then `/quality-review`.
4. Keep harness work (scripts, CI, gates) to about 20% of changes; ship product.

## Where to look

| Task | Read |
|---|---|
| Where code belongs, a new crate or dependency | `architecture` skill |
| Anything touching S3 | `s3-cost` skill |
| Porting from Java | [docs/porting.md](docs/porting.md) and the crate's `port-map.tsv` |
| REST behaviour and compatibility | [docs/compatibility.md](docs/compatibility.md) |
| A setting, endpoint, extension point | [docs/how-to/](docs/how-to/README.md) |
| Tests; optimising; `unsafe` and SIMD | `test-coverage`, `rust-performance`, `unsafe-code` skills |
| Committing, PRs, merging | `git-workflow` skill |
| A decision with trade-offs | an ADR in [docs/adr/](docs/adr/README.md); only the owner accepts one |
| What must never break, and its tests | [docs/invariants.md](docs/invariants.md) |
| Inside one crate | its `AGENTS.md` and `README.md` |
| Skills, this file, any new rule | `manage-skills` skill |
| Replies and reports | `brevity` skill |
| Why a practice is or is not here | [docs/harness-review.md](docs/harness-review.md) |
