#!/usr/bin/env python3
"""Every Rust source file is traceable to the Java it ports, or says why not.

Each crate has its own crates/<name>/port-map.tsv (one per crate, so parallel
work on different crates never edits the same file), with columns rust_path,
kind, java_source, note. rust_path is relative to the crate directory.
- Every .rs file under crates/<name>/src has exactly one row in that crate's map.
- Every row's rust_path exists.
- kind is `port` or `new`; a `port` row names its java_source (path@commit);
  a `new` row has a note saying why there is no Java counterpart.

Cannot catch: a java_source that does not exist at its commit (the Java tree
is not in this repository), or a port that drifted from its source.
"""
import sys

from checklib import ROOT, report, tracked

HEADER = "rust_path\tkind\tjava_source\tnote"


def main():
    hits = []
    for crate in sorted(p for p in ROOT.glob("crates/*") if p.is_dir()):
        rel_crate = crate.relative_to(ROOT).as_posix()
        mapfile = crate / "port-map.tsv"
        mrel = f"{rel_crate}/port-map.tsv"
        rows = {}
        if not mapfile.is_file():
            hits.append(f"{mrel}: missing")
        else:
            lines = mapfile.read_text(encoding="utf-8").splitlines()
            if not lines or lines[0].strip() != HEADER:
                hits.append(f"{mrel}:1: header must be '{HEADER}' (tab-separated)")
            for n, line in enumerate(lines[1:], 2):
                if not line.strip():
                    continue
                cols = line.split("\t")
                if len(cols) != 4:
                    hits.append(f"{mrel}:{n}: expected 4 tab-separated columns, got {len(cols)}")
                    continue
                rust, kind, java, note = (c.strip() for c in cols)
                if rust in rows:
                    hits.append(f"{mrel}:{n}: {rust} listed twice")
                rows[rust] = n
                if not (crate / rust).is_file():
                    hits.append(f"{mrel}:{n}: {rust} does not exist in {rel_crate}")
                if kind not in ("port", "new"):
                    hits.append(f"{mrel}:{n}: kind must be 'port' or 'new', got '{kind}'")
                elif kind == "port" and not java:
                    hits.append(f"{mrel}:{n}: a port row names its Java source")
                elif kind == "new" and not note:
                    hits.append(f"{mrel}:{n}: a new row says why there is no Java counterpart")
        prefix = f"{rel_crate}/src/"
        for rel in tracked({".rs"}):
            if rel.startswith(prefix) and rel[len(rel_crate) + 1:] not in rows:
                hits.append(f"{rel}: no row in {mrel} (port with its Java source, or new with a reason)")
    return report("check-port-map", hits)


if __name__ == "__main__":
    sys.exit(main())
