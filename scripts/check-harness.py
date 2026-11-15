#!/usr/bin/env python3
"""The agent harness stays loadable, small and pointed at things that exist.

- `.claude/skills` resolves to `.agents/skills`, so Claude Code lists the
  repo's skills (on a Windows checkout without symlinks git writes the link
  as a one-line file; that is accepted).
- Every `.agents/skills/<name>/SKILL.md` has frontmatter of exactly `name`
  (equal to the directory) and `description` (`WHAT: ... USE WHEN: ...`), a
  non-empty `## Enforced by` section, and at most 100 lines.
- Every "`<name>` skill" mentioned in Markdown names a skill that exists,
  and every `scripts/...` path mentioned in Markdown or a workflow exists
  (docs/harness-review.md is exempt: it names other repositories' files).
- Root AGENTS.md stays under its byte budget, a crate's AGENTS.md under its.
- Every check run by `scripts/tree-checks.sh` has a row in
  docs/mechanical-gates.md, and every `scripts/check-*` is run by something
  (tree-checks.sh, gate.sh, a hook or a workflow).

Cannot catch: a description that triggers at the wrong moment, a rule stated
twice in different words, or a skill whose body is wrong.
"""
import re
import sys

from checklib import ROOT, read, report, tracked

ROOT_BUDGET = 5500   # bytes; AGENTS.md is read in every session
CRATE_BUDGET = 1200  # bytes; a crate's AGENTS.md is read when working there
MAX_LINES = 100
# Discusses other repositories' skills and scripts by name; they are not this repo's.
FOREIGN_OK = {"docs/harness-review.md"}

FRONTMATTER = re.compile(r"\A---\n(.*?)\n---\n", re.S)
DESCRIPTION = re.compile(r'^"?WHAT: .+\. USE WHEN: .+"?$')
SKILL_REF = re.compile(r"`([a-z][a-z0-9-]*)` skill")
SCRIPT_REF = re.compile(r"\bscripts/[A-Za-z0-9_./-]*[A-Za-z0-9_]")
CHECK_IN_LIST = re.compile(r"^\s*(check-[a-z-]+\.py|selftest-checks\.py)", re.M)


def skills_link(hits):
    link = ROOT / ".claude/skills"
    if link.is_symlink() or link.is_dir():
        if link.resolve() != (ROOT / ".agents/skills").resolve():
            hits.append(".claude/skills: must resolve to .agents/skills")
    elif link.is_file():
        if read(".claude/skills").strip() != "../.agents/skills":
            hits.append(".claude/skills: must be a symlink to ../.agents/skills")
    else:
        hits.append(".claude/skills: missing; symlink it to ../.agents/skills so the skills load")


def skill_files(hits):
    names = set()
    for skill in sorted(p for p in (ROOT / ".agents/skills").glob("*") if p.is_dir()):
        rel = f".agents/skills/{skill.name}/SKILL.md"
        names.add(skill.name)
        if not (ROOT / rel).is_file():
            hits.append(f"{rel}: missing")
            continue
        text = read(rel)
        m = FRONTMATTER.match(text)
        if not m:
            hits.append(f"{rel}: no --- frontmatter ---")
            continue
        fields = dict(line.split(":", 1) for line in m.group(1).splitlines() if ":" in line)
        fields = {k.strip(): v.strip() for k, v in fields.items()}
        if set(fields) != {"name", "description"}:
            hits.append(f"{rel}: frontmatter must be exactly name and description, got {sorted(fields)}")
        if fields.get("name") != skill.name:
            hits.append(f"{rel}: name must be '{skill.name}'")
        if not DESCRIPTION.match(fields.get("description", "")):
            hits.append(f"{rel}: description must read 'WHAT: <summary>. USE WHEN: <triggers>'")
        enforced = re.search(r"^## Enforced by[ \t]*\n(.*?)(?=^## |\Z)", text, re.M | re.S)
        if not enforced or not enforced.group(1).strip():
            hits.append(f"{rel}: needs a non-empty '## Enforced by' section")
        lines = text.count("\n")
        if lines > MAX_LINES:
            hits.append(f"{rel}: {lines} lines, over {MAX_LINES}; move detail to docs/")
    return names


def references(hits, skills):
    files = [p for p in tracked({".md", ".yml"})
             if p.endswith(".md") or p.startswith(".github/workflows/")]
    for rel in files:
        if rel in FOREIGN_OK:
            continue
        for n, line in enumerate(read(rel).splitlines(), 1):
            if rel.endswith(".md"):
                for name in SKILL_REF.findall(line):
                    if name not in skills:
                        hits.append(f"{rel}:{n}: no skill named '{name}'")
            for path in SCRIPT_REF.findall(line):
                if not (ROOT / path).exists():
                    hits.append(f"{rel}:{n}: {path} does not exist")


def budgets(hits):
    if (ROOT / "AGENTS.md").is_file():
        size = len(read("AGENTS.md").encode("utf-8"))
        if size > ROOT_BUDGET:
            hits.append(f"AGENTS.md: {size} bytes, over {ROOT_BUDGET}; move detail to a skill or docs/")
    for path in sorted(ROOT.glob("crates/*/AGENTS.md")):
        rel = path.relative_to(ROOT).as_posix()
        size = len(read(rel).encode("utf-8"))
        if size > CRATE_BUDGET:
            hits.append(f"{rel}: {size} bytes, over {CRATE_BUDGET}")


def strip_comments(text):
    """Shell and YAML text without its `#` comment lines, so a name in a comment does not count as run."""
    return "\n".join(line for line in text.splitlines() if not line.lstrip().startswith("#"))


def gates(hits):
    listed = set()
    if (ROOT / "scripts/tree-checks.sh").is_file():
        listed = set(CHECK_IN_LIST.findall(read("scripts/tree-checks.sh")))
    table = read("docs/mechanical-gates.md") if (ROOT / "docs/mechanical-gates.md").is_file() else ""
    for script in sorted(listed):
        if script != "selftest-checks.py" and f"`{script}" not in table:
            hits.append(f"docs/mechanical-gates.md: no row for {script}, which tree-checks.sh runs")
    runners = "\n".join(
        strip_comments(read(p)) for p in tracked()
        if p in ("scripts/tree-checks.sh", "scripts/gate.sh") or p.startswith((".githooks/", ".github/workflows/"))
    )
    for path in sorted((ROOT / "scripts").glob("check-*")):
        if path.name != "checklib.py" and path.name not in runners:
            hits.append(f"scripts/{path.name}: nothing runs it (tree-checks.sh, gate.sh, a hook or a workflow)")


def main():
    hits = []
    skills_link(hits)
    skills = skill_files(hits) if (ROOT / ".agents/skills").is_dir() else set()
    references(hits, skills)
    budgets(hits)
    gates(hits)
    return report("check-harness", hits)


if __name__ == "__main__":
    sys.exit(main())
