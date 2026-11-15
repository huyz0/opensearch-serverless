# 0010: One home per rule, and a harness that checks itself

**Status:** Proposed, 2026-11-15. The owner asked for every repo rule to be "token efficient and hard to miss in the same time, using progressive disclosure".

## Context

The same rules were written in AGENTS.md, docs/principles.md, a skill and a how-to (the S3 cost rules four times, the PR process three), and the copies had drifted: AGENTS.md forbade any PUT that saves LISTs while the owner's rule allows one PUT that saves many, and two crate notes claimed a high-risk status nothing enforced. Claude Code listed none of the repo's skills (no `.claude/skills` link) and never read a crate's `AGENTS.md` (it loads only `CLAUDE.md`). The no-build check list was copied into the gate, the pre-commit hook and CI, and had drifted too. "No AI attribution" was prose, while agent tools add attribution by default.

## Decision

- **Layers:** AGENTS.md (read every session) is one line per invariant, each pointing to the rule's home; skill descriptions say when to load the skill; a skill body is a procedure that links to the rule; `docs/` holds the rule in full. Rules are cited by name, never by number.
- **One home per rule:** engineering rules in `docs/principles.md`, process rules in the `git-workflow` skill, harness rules in the `manage-skills` skill.
- **A rule that can be a predicate over files is a check:** `scripts/check-harness.py` (skills load and are well formed, named skills and scripts exist, AGENTS.md byte budgets, every check documented and run), the crate `CLAUDE.md` adapter in `check-crate-layout.py`, and attribution refused by `check-commit-msg.sh` and `check-pr.py`. `scripts/tree-checks.sh` is the one list of no-build checks.
- Third-party dependency changes leave the high-risk list, by the owner's decision of 2026-11-15; `crates/meta` and `crates/mem` join it.

## Consequences

- Always-loaded text is bounded (AGENTS.md 5,500 bytes, a crate's 1,200) and shrank from 5,777 to about 4,900 bytes while gaining the attribution and fencing rules.
- Reading a skill sometimes costs one more hop to the rule's home; in exchange a rule changes in one place.
- Still not mechanical: a rule restated in other words, and whether a description triggers at the right moment. Review catches those.
