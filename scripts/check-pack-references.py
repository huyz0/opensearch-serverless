#!/usr/bin/env python3
"""Every file that can persist a reference to a node pack is registered.

The collector deletes a pack only when nothing names it, and a clone, restore
or split reaches a pack by copying a manifest or a file location, never by
calling the pack constructor. So the rule is over files, not constructors: a
source file under crates/*/src that names `CommitManifest`, `BucketManifest`
or `FileLocation` must be listed in docs/pack-reference-sources.tsv,
which the collector's reference check reads. A new persister fails here until
a person classifies it, and a listed file that no longer names one of them
fails too, so the list cannot rot.

Cannot catch: a persister that holds a reference as a bare string and names
none of the three types.
"""
import re
import sys

from checklib import read, report, tracked

LIST = "docs/pack-reference-sources.tsv"
NAMES = re.compile(r"\b(CommitManifest|BucketManifest|FileLocation)\b")
SRC = re.compile(r"^crates/[^/]+/src/.*\.rs$")
ROLES = {"source", "format"}


def listed():
    entries, hits = {}, []
    for n, line in enumerate(read(LIST).splitlines(), 1):
        if not line.strip() or line.startswith("#"):
            continue
        cols = line.split("\t")
        if len(cols) < 2 or cols[1] not in ROLES:
            hits.append(f"{LIST}:{n}: expected `path<TAB>source|format<TAB>note`")
            continue
        entries[cols[0]] = n
    return entries, hits


def main():
    entries, hits = listed()
    named = {rel for rel in tracked({".rs"}) if SRC.match(rel) and NAMES.search(read(rel))}
    for rel in sorted(named - entries.keys()):
        hits.append(f"{rel}: names a manifest or a file location but is not in {LIST}")
    for rel, n in sorted(entries.items()):
        if rel not in named:
            hits.append(f"{LIST}:{n}: {rel} no longer names a manifest or a file location, or is gone")
    return report("check-pack-references", hits)


if __name__ == "__main__":
    sys.exit(main())
