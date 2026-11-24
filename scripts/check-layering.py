#!/usr/bin/env python3
"""Crate dependencies point downward only.

Each workspace crate has a layer; a crate may depend only on crates in a lower
layer, so crates in the same layer (reader and writer) never depend on each
other. A crate missing from LAYERS fails, so a new crate is placed on purpose.
Crates in DEV_ONLY (test helpers) may appear only under [dev-dependencies],
and there any crate may name them, whatever its layer: a test helper is never
compiled into a shipped build, so it cannot make a shipped cycle.

Crates in STANDALONE are meant to move to their own repository: no manifest
anywhere under them (benches and fuzz packages included) may name a workspace
crate or a path outside the crate, in any section, dev-only or not.

Cannot catch: a dependency through a non-workspace crate, or one under a
`[target.*.dependencies]` table (none exist; extend the parser if one is added).
"""
import re
import sys

from checklib import ROOT, report

LAYERS = {
    "oss-types": 0,
    "oss-mem": 0,
    "oss-testkit": 0,
    "oss-blob": 1,
    "oss-format": 1,
    "oss-rest": 1,
    "oss-transport": 1,
    "oss-meta": 2,
    "oss-cluster": 2,
    "oss-doc": 2,
    "oss-reader": 3,
    "oss-writer": 3,
    "oss-janitor": 3,
    "oss-coordinator": 4,
    "oss-node": 5,
    "oss-compat": 6,
    "chunkjson": 0,
    "oss-ext": 0,
}
DEV_ONLY = {"oss-testkit"}
STANDALONE = ["crates/chunkjson"]
PATH_DEP = re.compile(r'path\s*=\s*"([^"]+)"')
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
        in_deps, section = False, ""
        for line in text.splitlines():
            if line.startswith("["):
                m_sec = SECTION.match(line)
                in_deps, section = bool(m_sec), (m_sec.group(1) if m_sec else "")
                continue
            m = DEP.match(line.strip()) if in_deps else None
            if not m:
                continue
            dep = m.group(1)
            if dep in DEV_ONLY and section != "dev-dependencies":
                hits.append(f"{name}: {dep} is test-only; name it under [dev-dependencies]")
                continue
            if dep in DEV_ONLY:
                continue
            if dep not in LAYERS:
                hits.append(f"{name}: depends on {dep}, which has no layer")
            elif LAYERS[dep] >= LAYERS[name]:
                hits.append(
                    f"{name} (layer {LAYERS[name]}) depends on {dep} (layer {LAYERS[dep]}): "
                    "dependencies point downward only"
                )
    for crate in STANDALONE:
        root = (ROOT / crate).resolve()
        for manifest in sorted((ROOT / crate).rglob("Cargo.toml")):
            if "target" in manifest.relative_to(ROOT).parts:
                continue
            rel = manifest.relative_to(ROOT).as_posix()
            for n, line in enumerate(manifest.read_text(encoding="utf-8").splitlines(), 1):
                path = PATH_DEP.search(line)
                outside = path and not (manifest.parent / path.group(1)).resolve().is_relative_to(root)
                if DEP.match(line.strip()) or outside:
                    hits.append(f"{rel}:{n}: {crate} is standalone; it may not depend on workspace crates or paths outside it")
    return report("check-layering", hits)


if __name__ == "__main__":
    sys.exit(main())
