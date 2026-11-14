---
name: manage-skills
description: "WHAT: Rules for this repo's agent skills and AGENTS.md. USE WHEN: creating or editing any file under .agents/skills/, .claude/, or AGENTS.md."
---

# Managing skills

## Rules

- **Pointer, not rulebook.** A `SKILL.md` routes to the doc that explains (`docs/`) and names the check that enforces it. Under 100 lines; the doc wins on conflict.
- **Frontmatter is the trigger:** exactly `name` (equal to the directory name) and `description`, in the form `WHAT: <summary>. USE WHEN: <concrete triggers>`, naming crates, paths or commands.
- **Bind to an enforcer.** Every skill has an "Enforced by" section; if nothing mechanical checks a rule, say so.
- **No vendor syntax in AGENTS.md or skills.** Claude Code's `@import` lives only in `CLAUDE.md`; `.claude/` files are thin adapters that point here.
- **A skill that cannot run is worse than none.** Add a skill when the thing it operates on exists.
- **Same change:** a change that alters a skill's subject updates the skill.

## Layout

```
.agents/skills/<name>/SKILL.md      one skill, one responsibility
.claude/skills -> ../.agents/skills  symlink so Claude Code finds them
```

## Enforced by

- Nothing mechanical yet; self-review.
