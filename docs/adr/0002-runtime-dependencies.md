# 0002: How hsearch, lucene-rust and pstore are consumed

**Status:** Proposed. Needs the owner's decision.

## Context

The assessment recommends hsearch as the single Rust runtime, lucene-rust as the engine (page cache, S3 range streaming, realtime search, encryption), and pstore's cost, catalog and meter pieces folded into hsearch. All three are separate repositories under active development. Copying them is ruled out.

| Option | For | Against |
|---|---|---|
| **Pinned git dependencies** (`git = ..., rev = "<sha>"`), the way hsearch already pins lucene-rust | Each repo keeps its own gate and history; a bump is a reviewed one-line diff; this repo builds alone | A change spanning two repos needs two PRs and a pin bump; local cross-repo edits need a `[patch]` override |
| **Workspace members** (git submodules or a monorepo) | One build, one change across repos | Inherits lucene-rust's 44 crates and 1.27M lines into every build and coverage run; three gates with different floors (95% vs 90%) must merge; submodules complicate cloud sandboxes |

## Proposed decision

Pinned git dependencies at exact revisions, with a commented `[patch]` block for local cross-repo work. Pull pstore pieces in through hsearch once they are folded there, not directly.

## Consequences

- This repo's gate and coverage cover only its own code.
- Cloud sandboxes need network access to GitHub at build time (a vendored `cargo vendor` directory is the fallback).
