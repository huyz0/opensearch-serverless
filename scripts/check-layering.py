#!/usr/bin/env python3
"""Crate dependencies point downward only.

Each workspace crate has a layer; a crate may depend only on crates in a lower
layer, so crates in the same layer (reader and writer) never depend on each
other. A crate missing from LAYERS fails, so a new crate is placed on purpose.

Cannot catch: a dependency through a non-workspace crate, or one under a
`[target.*.dependencies]` table (none exist; extend the parser if one is added).
"""
import re
import sys

from checklib import ROOT, report

LAYERS = {
    "oss-types": 0,
    "oss-blob": 1,
    "oss-reader": 2,
    "oss-writer": 2,
    "oss-coordinator": 3,
    "oss-node": 4,
}
SECTION = re.compile(r"^\[(dependencies|dev-dependencies|build-dependencies)\]\s*$")
DEP = re.compile(r"^(oss-[a-z0-9-]+)\s*=")


def main():
    hits = []
    for manifest in sorted(ROOT.glob("crates/*/Cargo.toml")):
        text = manifest.read_text(encoding="utf-8")
        name = re.search(r'^name\s*=\s*"([^"]+)"', text, re.M).group(1)
        if name not in LAYERS:
            hits.append(f"{manifest.relative_to(ROOT).as_posix()}: crate {name} has no layer in check-layering.py")
            continue
        in_deps = False
        for line in text.splitlines():
            if line.startswith("["):
                in_deps = bool(SECTION.match(line))
                continue
            m = DEP.match(line.strip()) if in_deps else None
            if not m:
                continue
            dep = m.group(1)
            if dep not in LAYERS:
                hits.append(f"{name}: depends on {dep}, which has no layer")
            elif LAYERS[dep] >= LAYERS[name]:
                hits.append(
                    f"{name} (layer {LAYERS[name]}) depends on {dep} (layer {LAYERS[dep]}): "
                    "dependencies point downward only"
                )
    return report("check-layering", hits)


if __name__ == "__main__":
    sys.exit(main())
