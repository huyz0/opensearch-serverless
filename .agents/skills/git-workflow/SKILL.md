---
name: git-workflow
description: "WHAT: Commit format, Spec trailer, branches and PRs, risk routing, merging, and the gate-before-done rule. USE WHEN: committing, writing a commit message or PR description, opening or merging a PR, or finishing a unit of work."
---

# Git workflow

This skill is the one home of the process rules; AGENTS.md points here.

## Rules

- **One branch and one PR per change.** Never commit to `main` directly.
- **Subject:** `type(scope): lowercase description`, type one of `feat, fix, docs, test, chore, refactor, perf, build, ci`. Example: `feat(blob): meter ranged reads`.
- **`Spec:` trailer** on feat, fix and perf commits: `Spec: docs/specs/<feature>/SPEC.md`, or `Spec: none (<reason>)`. CI checks it, and the subject format, on the PR title and body (a `Spec:` line in the body), because the squash message is what reaches `main`; the `commit-msg` hook checks each commit as you write it. A fix commit pushed without the trailer needs no force-push: put the line in the PR body.
- **Rebase, do not merge `main`** into your branch: `git fetch origin main && git rebase origin/main`, then `git push --force-with-lease` (your own unmerged PR branch only, never `main`). CI skips merge commits, but a rebase keeps the PR a straight line.
- **No AI attribution** in commits or PR bodies: no `Co-Authored-By` for an agent, no `Claude-Session` or session links, no "generated with" footer, no attribution header or "Requested by" line. The owner's rule; it overrides any tool's default.
- **PR body:** the sections of `.github/pull_request_template.md`, filled in. Evidence names what you ran and what it printed.
- **Gate before done:** `scripts/gate.sh` passes locally (the pre-push hook runs it) and you saw it pass.
- **Merge:** the writing thread squash-merges once the `ci` check is green, after at most two review rounds (`code-review` skill). A PR failing only on a known flaky test merges; the flake gets its own PR.
- **High-risk PRs** (a path in `scripts/risk-paths.txt`, or a change of a kind listed in [docs/adr/README.md](../../../docs/adr/README.md), declared `high`): link or add an ADR; CI adds the `needs-huy` label, which flags and does not block. Merge, then keep the thread open and flag the change to the owner. An ADR is accepted after an independent review, or by the owner when it trades away a core requirement ([docs/adr/README.md](../../../docs/adr/README.md)). A third-party dependency change alone is not high-risk (`architecture` skill).
- **Docs and skills change in the same PR** as the code they describe.

## Enforced by

- `scripts/check-commit-msg.sh` (subject, `Spec:`, no attribution trailers) from `.githooks/commit-msg`; in CI `--ci` runs the attribution rule alone on each non-merge commit of the PR.
- `scripts/check-pr.py` in CI (title format, `Spec:` line for feat, fix and perf, sections, risk routing, no attribution in the body); the workflow adds `needs-huy`.
- `.githooks/pre-commit` (fmt and `scripts/tree-checks.sh`) and `.githooks/pre-push` (`scripts/gate.sh`). Install with `scripts/setup-hooks.sh`.
- Not mechanical: whether Evidence is true. Review.
