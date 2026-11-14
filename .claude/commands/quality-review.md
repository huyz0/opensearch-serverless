---
description: Run the independent semantic review on the current diff via the quality-reviewer subagent.
argument-hint: "[optional git range, e.g. HEAD~3..HEAD]"
allowed-tools: Task, Bash(git diff:*), Bash(git status:*), Read, Grep, Glob
---

Review target: $ARGUMENTS (empty means the working tree plus staged changes).

Spawn the `quality-reviewer` subagent with the target and a one-line statement of the change's intent, and nothing else about why it was written that way. It follows `.agents/skills/code-review/SKILL.md`. Relay its findings, BLOCKING separated from MINOR.
