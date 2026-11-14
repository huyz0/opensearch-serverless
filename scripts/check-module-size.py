#!/usr/bin/env python3
"""Small modules, with a ratchet.

- A Rust file over 1,000 lines fails.
- A Rust file over 500 lines fails unless it is listed in
  scripts/size-baseline.txt, and then only while it is no longer than its
  listed length. The baseline only shrinks: split a file, then lower or
  remove its line. Adding a line to the baseline is a reviewed decision.

Cannot catch: a short file that mixes concerns.
"""
import sys

from checklib import ROOT, read, report, tracked

SOFT, HARD = 500, 1000
BASELINE = "scripts/size-baseline.txt"


def baseline():
    path = ROOT / BASELINE
    allowed = {}
    if path.is_file():
        for line in path.read_text(encoding="utf-8").splitlines():
            line = line.split("#", 1)[0].strip()
            if line:
                rel, n = line.rsplit(maxsplit=1)
                allowed[rel] = int(n)
    return allowed


def main():
    hits, allowed = [], baseline()
    for rel in tracked({".rs"}):
        n = len(read(rel).splitlines())
        if n > HARD:
            hits.append(f"{rel}: {n} lines (limit {HARD})")
        elif n > SOFT and rel not in allowed:
            hits.append(f"{rel}: {n} lines (limit {SOFT}); split it along a concern")
        elif rel in allowed and n > allowed[rel]:
            hits.append(f"{rel}: {n} lines, baseline {allowed[rel]}; the baseline only shrinks")
    return report("check-module-size", hits)


if __name__ == "__main__":
    sys.exit(main())
