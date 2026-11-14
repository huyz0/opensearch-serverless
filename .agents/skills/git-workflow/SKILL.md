---
name: git-workflow
description: "WHAT: Commit format, Spec trailer, branches and PRs, risk routing, the gate-before-done rule, and hooks. USE WHEN: committing, writing a commit message or PR description, opening or merging a PR, or finishing a unit of work."
---

# Git workflow

## Rules

- **One branch and one PR per change.** Do not commit to `main` directly.
- **Subject:** `type(scope): lowercase description`, type one of `feat, fix, docs, test, chore, refactor, perf, build, ci`. Example: `feat(blob): meter ranged reads`.
- **`Spec:` trailer** on feat, fix and perf commits: `Spec: docs/specs/<feature>/SPEC.md`, or `Spec: none (<reason>)`.
- **No AI attribution:** no `Co-Authored-By` for an agent, no session links, no "generated with" footers, in commits or PR bodies.
- **PR body:** the sections of `.github/pull_request_template.md`, filled in. Evidence names what you ran and what it printed.
- **Merge:** the writing thread merges (squash) once the `ci` check is green. A PR failing only on a known flaky test merges; fix the flake in a separate PR.
- **High-risk PRs** (paths in `scripts/risk-paths.txt`, or declared high): ADR linked or added; CI adds the `needs-huy` label. Merge, then keep the thread open and flag the change to the owner for review.
- **Gate before done:** `scripts/gate.sh` passes locally before pushing.
- **Docs and skills change in the same PR** as the code they describe.

## Enforced by

- `.githooks/commit-msg` and CI's `ci` workflow, both through `scripts/check-commit-msg.sh` (subject, `Spec:`).
- `.githooks/pre-commit` (the gate). Install with `scripts/setup-hooks.sh`.
- `scripts/check-pr.py` in the `ci` workflow (sections, risk routing); the workflow adds `needs-huy` to high-risk PRs.
- Not mechanical: whether Evidence is true. Review.
