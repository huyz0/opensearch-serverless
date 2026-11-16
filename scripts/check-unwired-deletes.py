#!/usr/bin/env python3
"""Index deletes and reclaims are wired to no entry point yet (ADR 0012).

`IndexLifecycle::delete_index`, `IndexLifecycle::reclaim` and
`ReclaimQueue::drain` need real shard heads and snapshot pins: without them a
delete purges bytes a Java shallow snapshot names, and a drain leaves a Java
writer's heads. Fails on `IndexLifecycle` or `ReclaimQueue` named in a crate's
`src/` outside `crates/meta`, comments aside: what is never named is never
called. The change that ports heads and pins and wires the delete
retires this check, with ADR 0012's consequences.

Cannot catch: a type reached through a renamed re-export.
"""
import re
import sys

from checklib import read, report, tracked

NAMED = re.compile(r"\b(IndexLifecycle|ReclaimQueue)\b")


def main():
    hits = []
    for rel in tracked({".rs"}):
        if not rel.startswith("crates/") or rel.startswith("crates/meta/") or "/src/" not in rel:
            continue
        for n, line in enumerate(read(rel).splitlines(), 1):
            if not line.lstrip().startswith("//") and NAMED.search(line):
                hits.append(f"{rel}:{n}: the index lifecycle wired before shard heads and pins exist (ADR 0012)")
    return report("check-unwired-deletes", hits)


if __name__ == "__main__":
    sys.exit(main())
