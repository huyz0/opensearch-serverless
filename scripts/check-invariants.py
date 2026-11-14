#!/usr/bin/env python3
"""Every invariant in docs/invariants.md names evidence that exists.

Each table row `| I-<n> | statement | evidence |` must name, in backticks, at
least one existing check script or test function, and every test name it
gives must resolve to a `#[test]` fn under crates/. A row whose evidence is
`NOT-YET: <reason>` passes, so an unenforced invariant is stated, not hidden.

Cannot catch: evidence that exists but does not test the invariant.
"""
import re
import sys

from checklib import ROOT, report, test_names, unresolved_evidence

DOC = "docs/invariants.md"
ROW = re.compile(r"^\|\s*(I-\d+)\s*\|(.*)\|(.*)\|\s*$")


def main():
    path = ROOT / DOC
    if not path.is_file():
        return report("check-invariants", [f"{DOC}: missing"])
    tests, hits, seen = test_names(), [], set()
    for n, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        m = ROW.match(line)
        if not m:
            continue
        inv, evidence = m.group(1), m.group(3)
        if inv in seen:
            hits.append(f"{DOC}:{n}: {inv} is listed twice")
        seen.add(inv)
        if re.search(r"NOT-YET:\s*\S", evidence):
            continue
        ok, bad = unresolved_evidence(evidence, tests)
        for item in bad:
            hits.append(f"{DOC}:{n}: {inv} names `{item}`, which is no test or path")
        if ok == 0:
            hits.append(f"{DOC}:{n}: {inv} names no evidence (a check, a test, or NOT-YET: reason)")
    if not seen:
        hits.append(f"{DOC}: no invariant rows found")
    return report("check-invariants", hits)


if __name__ == "__main__":
    sys.exit(main())
