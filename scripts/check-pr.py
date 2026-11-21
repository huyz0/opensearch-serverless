#!/usr/bin/env python3
"""Checks a pull request's description and risk routing.

  check-pr.py [--title FILE] --body FILE --labels "a,b" --changed FILE --added FILE

- The title (the squash commit's subject) is `type(scope): lowercase
  description`. For feat, fix and perf the body has a line
  `Spec: docs/specs/<feature>/SPEC.md` naming an existing file, or
  `Spec: none (<reason>)`. Commits inside the PR are not checked for either:
  they never reach main.
- The body has every section of .github/pull_request_template.md, each with
  text beyond the template's comments.
- A PR is high-risk when a changed path matches scripts/risk-paths.txt, it
  adds a crate (a new crates/*/Cargo.toml), or
  its Risk tier section says `high`. A high-risk PR must link an ADR
  (`docs/adr/NNNN-...`) in its body or add one, must say `high` in Risk tier,
  and gets the `needs-huy` label (the pr workflow adds it). The label does
  not block the merge: it flags the change for the owner's review after it
  lands.

- The body carries no AI attribution: no "Generated with Claude Code"
  footer, no session link, no `claude-projects-attribution` header and no
  "Requested by" line (the owner's rule for every repository).

Cannot catch: a public API change in a path not listed as risky; declare
those `high` by hand.
"""
import argparse
import fnmatch
import re
import sys

from checklib import ROOT, report

SECTIONS = ["Intent", "Alternatives rejected", "Invariants", "Evidence", "Risk tier", "Where to look"]
ATTRIBUTION = re.compile(
    r"Generated with \[?Claude|claude\.ai/code/session|claude-projects-attribution|^_?Requested by \*\*",
    re.I | re.M)
SUBJECT = re.compile(r"(feat|fix|docs|test|chore|refactor|perf|build|ci)(\([a-z0-9-]+\))?: [a-z].*")
LABEL = "needs-huy"  # a review flag, not a merge block


def sections(body):
    body = re.sub(r"<!--.*?-->", "", body, flags=re.S)
    found = {}
    for m in re.finditer(r"^## (.+?)\s*\n(.*?)(?=^## |\Z)", body, re.M | re.S):
        found[m.group(1).strip()] = m.group(2).strip()
    return found


def spec_hits(title, body):
    if not SUBJECT.fullmatch(title):
        return [f"PR title: must be 'type(scope): lowercase description', got: {title}"]
    if not re.match(r"(feat|fix|perf)\b", title):
        return []
    m = re.search(r"^Spec: (.+?)\s*$", body, re.M)
    if not m:
        return ["PR body: feat, fix and perf need a line 'Spec: docs/specs/<feature>/SPEC.md' or 'Spec: none (<reason>)'"]
    spec = m.group(1)
    if not re.fullmatch(r"none \(.+\)", spec) and not (ROOT / spec).is_file():
        return [f"PR body: Spec '{spec}' is not an existing file or 'none (<reason>)'"]
    return []


def risk_rules():
    rules = []
    for line in (ROOT / "scripts/risk-paths.txt").read_text(encoding="utf-8").splitlines():
        line = line.split("#", 1)[0].strip()
        if line:
            glob, _, reason = line.partition(" ")
            rules.append((glob, reason.strip()))
    return rules


def matches(path, glob):
    if fnmatch.fnmatchcase(path, glob):
        return True
    return glob.startswith("**/") and fnmatch.fnmatchcase(path, glob[3:])


def risky(changed):
    reasons = set()
    for path in changed:
        for glob, reason in risk_rules():
            if matches(path, glob):
                reasons.add(f"{path} ({reason})")
    return sorted(reasons)


def lines_of(path):
    if not path:
        return []
    with open(path, encoding="utf-8") as f:
        return [line.strip() for line in f if line.strip()]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--title")
    ap.add_argument("--body", required=True)
    ap.add_argument("--labels", default="")
    ap.add_argument("--changed")
    ap.add_argument("--added")
    args = ap.parse_args()
    with open(args.body, encoding="utf-8") as f:
        body = f.read()
    labels = {label.strip() for label in args.labels.split(",") if label.strip()}
    changed, added = lines_of(args.changed), lines_of(args.added)

    hits = []
    if args.title:
        with open(args.title, encoding="utf-8") as f:
            title = f.readline().strip()
        hits += spec_hits(title, body)
    found = sections(body)
    for sec in SECTIONS:
        if sec not in found:
            hits.append(f"PR body: missing section '## {sec}'")
        elif not found[sec]:
            hits.append(f"PR body: section '## {sec}' is empty")
    if ATTRIBUTION.search(body):
        hits.append("PR body: remove the AI attribution (footer, session link, header or 'Requested by' line)")
    tier = found.get("Risk tier", "").lower()
    reasons = risky(changed)
    reasons += [f"{p} (new workspace crate)" for p in added if re.fullmatch(r"crates/[^/]+/Cargo\.toml", p)]
    high = bool(reasons) or re.search(r"\bhigh\b", tier)
    if high:
        if reasons:
            print("high-risk paths:\n  " + "\n  ".join(reasons))
            if not re.search(r"\bhigh\b", tier):
                hits.append("Risk tier: these paths make the PR high-risk; say 'high'")
        adr_linked = re.search(r"docs/adr/\d{4}-", body)
        adr_added = any(re.match(r"docs/adr/\d{4}-.*\.md$", p) for p in added)
        if not (adr_linked or adr_added):
            hits.append("high-risk PR: link an ADR (docs/adr/NNNN-...) or add one")
        if LABEL not in labels:
            print(f"high-risk PR: the '{LABEL}' label flags it for the owner's review")
    return report("check-pr", hits)


if __name__ == "__main__":
    sys.exit(main())
