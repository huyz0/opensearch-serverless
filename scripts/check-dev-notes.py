#!/usr/bin/env python3
"""Code comments say what the code does, never when or why it was planned.

Fails on milestone, phase, stage or task references in comments (`M12`,
`T3.4`, `phase 2`, `R1`, `task #12`, `PR #40`). History belongs in git and
docs/. Comments in .rs, .toml, .py, .sh and .yml files are scanned; docs/ and
Markdown are not.

Cannot catch: a process note in plain words ("added for the demo").
"""
import pathlib
import re
import sys

from checklib import read, report, tracked

SLASH = re.compile(r"^\s*(//|/\*|\*)")
HASH = re.compile(r"^\s*#(?!!|\[)")
PATTERNS = [
    re.compile(r"\bM\d{1,3}(?:\.\d+)?\b"),
    re.compile(r"\bT\d{1,2}\.\d{1,2}\b"),
    re.compile(r"\b(?:milestone|phase|stage|sprint)\s+[A-Z]?\d+\b", re.I),
    re.compile(r"\bR[0-9]\b"),
    re.compile(r"\b(?:task|PR|pull request|issue)\s*#\d+", re.I),
]
EXEMPT = {"scripts/check-dev-notes.py"}


def is_comment(ext, line):
    if ext == ".rs":
        return bool(SLASH.match(line))
    return bool(HASH.match(line))


def main():
    hits = []
    for rel in tracked({".rs", ".toml", ".py", ".sh", ".yml", ".yaml"}):
        if rel in EXEMPT or rel.startswith("docs/"):
            continue
        ext = pathlib.Path(rel).suffix
        for n, line in enumerate(read(rel).splitlines(), 1):
            if is_comment(ext, line) and any(p.search(line) for p in PATTERNS):
                hits.append(f"{rel}:{n}: {line.strip()}")
    return report("check-dev-notes", hits)


if __name__ == "__main__":
    sys.exit(main())
