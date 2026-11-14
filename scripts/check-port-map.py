#!/usr/bin/env python3
"""Every Rust source file is traceable to the Java it ports, or says why not.

docs/port-map.tsv has columns rust_path, kind, java_source, note.
- Every .rs file under crates/*/src has exactly one row.
- Every row's rust_path exists.
- kind is `port` or `new`; a `port` row names its java_source; a `new` row
  has a note saying why there is no Java counterpart.

Cannot catch: a java_source that does not exist at the pinned commit (the
Java tree is not in this repository), or a port that drifted from its source.
"""
import re
import sys

from checklib import ROOT, report, tracked

MAP = "docs/port-map.tsv"
SRC = re.compile(r"^crates/[^/]+/src/.+\.rs$")


def main():
    path = ROOT / MAP
    if not path.is_file():
        return report("check-port-map", [f"{MAP}: missing"])
    hits, rows = [], {}
    lines = path.read_text(encoding="utf-8").splitlines()
    for n, line in enumerate(lines[1:], 2):
        if not line.strip():
            continue
        cols = line.split("\t")
        if len(cols) != 4:
            hits.append(f"{MAP}:{n}: expected 4 tab-separated columns, got {len(cols)}")
            continue
        rust, kind, java, note = (c.strip() for c in cols)
        if rust in rows:
            hits.append(f"{MAP}:{n}: {rust} listed twice")
        rows[rust] = n
        if not (ROOT / rust).is_file():
            hits.append(f"{MAP}:{n}: {rust} does not exist")
        if kind not in ("port", "new"):
            hits.append(f"{MAP}:{n}: kind must be 'port' or 'new', got '{kind}'")
        elif kind == "port" and not java:
            hits.append(f"{MAP}:{n}: a port row names its Java source")
        elif kind == "new" and not note:
            hits.append(f"{MAP}:{n}: a new row says why there is no Java counterpart")
    for rel in tracked({".rs"}):
        if SRC.match(rel) and rel not in rows:
            hits.append(f"{rel}: no row in {MAP} (port with its Java source, or new with a reason)")
    return report("check-port-map", hits)


if __name__ == "__main__":
    sys.exit(main())
