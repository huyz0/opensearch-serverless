#!/usr/bin/env python3
"""`unsafe` stays forbidden outside the crates listed in ALLOWED (none today).

Fails on any attempt to relax the workspace's `unsafe_code = "forbid"`: an
`allow`/`expect`/`warn` of `unsafe_code` in Rust, or an `unsafe_code` lint
level other than "forbid" in a manifest. Adding a crate to ALLOWED needs an
ADR and an entry in docs/principles.md.

Cannot catch: unsafe code in a dependency.
"""
import re
import sys

from checklib import read, report, tracked

ALLOWED = set()  # crate directories, e.g. "crates/module-native"
RELAX_RS = re.compile(r"#!?\[\s*(?:allow|expect|warn)\s*\([^)]*\bunsafe_code\b")
RELAX_TOML = re.compile(r"""^\s*unsafe_code\s*=(?!\s*"forbid")""")


def crate_of(rel):
    parts = rel.split("/")
    return "/".join(parts[:2]) if parts[0] == "crates" and len(parts) > 2 else ""


def main():
    hits = []
    for rel in tracked({".rs", ".toml"}):
        if crate_of(rel) in ALLOWED and crate_of(rel):
            continue
        pattern = RELAX_RS if rel.endswith(".rs") else RELAX_TOML
        for n, line in enumerate(read(rel).splitlines(), 1):
            if pattern.search(line):
                hits.append(f"{rel}:{n}: relaxes unsafe_code; unsafe is forbidden outside {sorted(ALLOWED) or 'every crate'}")
    return report("check-unsafe", hits)


if __name__ == "__main__":
    sys.exit(main())
