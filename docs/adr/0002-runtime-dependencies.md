# 0002: lucene-rust is the only code dependency; hsearch and pstore are references

**Status:** Accepted by the owner, 2026-11-14. Amended 2026-11-15 by the owner ("You decide on crates deps dont ask me"): the decision limits only the sibling projects; other crates are the writing thread's choice.

## Context

The assessment of 2026-11-14 weighed hsearch as a Rust runtime and pstore's cost and catalog crates. The owner decided against using their code: this repository ports from OpenSearch and the Java serverless shell, and hsearch and pstore are references for ideas and engineering practice, in both directions. lucene-rust is the engine (page cache, S3 range streaming, realtime search, encryption).

For lucene-rust, two ways to consume it were weighed:

| Option | For | Against |
|---|---|---|
| **Pinned git dependency** (`git = ..., rev = "<sha>"`) | lucene-rust keeps its own gate and history; a bump is a reviewed one-line diff; this repo builds alone | A change spanning both repos needs two PRs and a pin bump; local cross-repo edits need a `[patch]` override |
| Workspace member (submodule or monorepo) | One build, one change across repos | Puts lucene-rust's 44 crates and 1.27M lines into every build and coverage run; two gates must merge; submodules complicate cloud sandboxes |

## Decision

- **lucene-rust** is consumed as pinned git dependencies at an exact revision, with a commented `[patch]` block for local cross-repo work. Only the crates a role needs are named.
- **hsearch and pstore** are not dependencies, and their code is not copied. Ideas taken from them are re-implemented here and noted in [harness-review.md](../harness-review.md); lessons found here are offered back.

- **Other crates** (crates.io or git) are chosen by the thread writing the change, without an ADR or the owner's review. Each must be pinned (exact version in `Cargo.lock`, or a git `rev`), maintained, minimal (what it pulls into `Cargo.lock` is checked) and Apache-2.0 compatible, and the PR says so.

## Consequences

- This repo's gate and coverage cover only its own code.
- Cloud sandboxes need network access to GitHub at build time (a `cargo vendor` directory is the fallback).
- Features hsearch already has (an S3 adapter, an extension ABI) are ported or re-designed here, informed by hsearch's design but owned by this repo.
