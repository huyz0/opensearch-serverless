---
name: manage-skills
description: "WHAT: Rules for this repo's agent harness: AGENTS.md, skills, adapters. USE WHEN: creating or editing any file under .agents/skills/ or .claude/, any AGENTS.md or CLAUDE.md, or adding a rule anywhere."
---

# Managing skills and rules

## Layers

| Layer | What | Loaded |
|---|---|---|
| 0 | `AGENTS.md`: one line per invariant, each pointing to its home | every session |
| 1 | each skill's `description` | every session |
| 2 | a skill's body: the procedure | when the skill is used |
| 3 | `docs/` and crate `AGENTS.md`: the rules in full, rationale, numbers | when a skill or link says so |

## Rules

- **One home per rule.** Product and engineering rules live in `docs/principles.md`, process rules in the `git-workflow` skill, harness rules here. Everywhere else links to the home instead of restating it; a restatement drifts.
- **Mechanical first.** A rule that can be a predicate over files gets a check in `scripts/` (listed in `scripts/tree-checks.sh`, with a row and a planted violation in `docs/mechanical-gates.md` and `scripts/selftest-checks.py`), not a sentence asking an agent to remember.
- **Pointer, not rulebook.** A `SKILL.md` is a procedure that links to the rule's home and names the check. Under 100 lines.
- **Frontmatter is the trigger:** exactly `name` (equal to the directory) and `description`, as `WHAT: <summary>. USE WHEN: <concrete triggers>`, naming crates, paths or commands. "Use when" beats "what it is".
- **Bind to an enforcer:** every skill has `## Enforced by`; if nothing mechanical checks a rule, say so.
- **Cite by name, never by number.** A numbered cross-reference goes stale silently when a list changes.
- **No vendor syntax in AGENTS.md or skills.** Claude Code's `@import` lives only in `CLAUDE.md` files, which are adapters: the root one and each crate's are exactly `@AGENTS.md`. `.claude/` holds thin adapters that point here.
- **A skill that cannot run is worse than none.** Add one when the thing it operates on exists.

## Layout

```
.agents/skills/<name>/SKILL.md       one skill, one responsibility
.claude/skills -> ../.agents/skills   symlink, so Claude Code lists them
crates/<name>/CLAUDE.md               @AGENTS.md, so Claude Code reads the crate's rules
```

## Enforced by

- `scripts/check-harness.py`: the symlink, frontmatter, `Enforced by`, 100 lines, skill and script references resolve, AGENTS.md byte budgets, every tree check documented and every check script run.
- `scripts/check-crate-layout.py`: each crate's `AGENTS.md` and `CLAUDE.md`.
- Not mechanical: a rule stated twice in different words, or a description that triggers at the wrong moment. Review.
