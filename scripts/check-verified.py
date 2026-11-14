#!/usr/bin/env python3
"""Every acceptance criterion in a spec has evidence that resolves.

For each docs/specs/<feature>/SPEC.md, the criteria are the lines starting
`- AC-<n>:` under `## Acceptance criteria`. When the feature's VERIFIED.md
exists (the feature claims to be done), it needs one line per criterion,
`- AC-<n>: ...`, naming a test function or a command in backticks, or saying
`NOT-RUN: <reason>`. Every test it names must resolve. A spec without a
VERIFIED.md is work in progress and passes.

Cannot catch: whether the evidence is true, or whether a named command
demonstrates the criterion. That is what review and the PR's Evidence
section are for.
"""
import re
import sys

from checklib import ROOT, report, test_names, unresolved_evidence

AC = re.compile(r"^\s*-\s*(AC-\d+)\s*:(.*)$")
COMMAND = re.compile(r"`(?:scripts/|cargo )[^`]+`")


def criteria(text):
    section = re.search(r"^## Acceptance criteria\s*\n(.*?)(?=^## |\Z)", text, re.M | re.S)
    return [m.group(1) for m in map(AC.match, (section.group(1) if section else "").splitlines()) if m]


def main():
    hits, tests = [], None
    for spec in sorted(ROOT.glob("docs/specs/*/SPEC.md")):
        rel = spec.relative_to(ROOT).as_posix()
        acs = criteria(spec.read_text(encoding="utf-8"))
        if not acs:
            hits.append(f"{rel}: no '- AC-<n>:' lines under '## Acceptance criteria'")
            continue
        verified = spec.with_name("VERIFIED.md")
        if not verified.is_file():
            continue
        tests = tests if tests is not None else test_names()
        lines = {}
        for line in verified.read_text(encoding="utf-8").splitlines():
            m = AC.match(line)
            if m:
                lines[m.group(1)] = m.group(2)
        vrel = verified.relative_to(ROOT).as_posix()
        for ac in acs:
            body = lines.get(ac)
            if body is None:
                hits.append(f"{vrel}: no evidence line for {ac}")
                continue
            if re.search(r"NOT-RUN:\s*\S", body):
                continue
            ok, bad = unresolved_evidence(body, tests)
            for item in bad:
                hits.append(f"{vrel}: {ac} names `{item}`, which is no test or path")
            if ok == 0 and not COMMAND.search(body):
                hits.append(f"{vrel}: {ac} names no test, command or NOT-RUN reason")
    return report("check-verified", hits)


if __name__ == "__main__":
    sys.exit(main())
