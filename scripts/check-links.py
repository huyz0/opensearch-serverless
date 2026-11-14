#!/usr/bin/env python3
"""Every relative link in a tracked Markdown file points at a path that exists.

Cannot catch: a broken #anchor, or a broken external URL.
"""
import re
import sys

from checklib import ROOT, read, report, tracked

LINK = re.compile(r"\]\(([^)\s]+)\)")
EXTERNAL = re.compile(r"^(?:[a-z]+:|#)")


def main():
    hits = []
    for rel in tracked({".md"}):
        base = (ROOT / rel).parent
        in_code = False
        for n, line in enumerate(read(rel).splitlines(), 1):
            if line.lstrip().startswith("```"):
                in_code = not in_code
            if in_code:
                continue
            for target in LINK.findall(line):
                path = target.split("#", 1)[0]
                if not EXTERNAL.match(target) and path and not (base / path).exists():
                    hits.append(f"{rel}:{n}: broken link {target}")
    return report("check-links", hits)


if __name__ == "__main__":
    sys.exit(main())
