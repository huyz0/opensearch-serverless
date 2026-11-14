#!/usr/bin/env python3
"""Every crate has the same shape, so a reader knows where to look.

crates/<name>/ holds Cargo.toml, src/lib.rs, an AGENTS.md with the crate's
local rules, and a README.md with non-empty
sections `## Owns`, `## Depends on` and `## Entry points`. The manifest opts
into the workspace lints (`[lints]` then `workspace = true`), which is what
makes a missing doc comment or an `unsafe` block a compile error.

Cannot catch: a README whose sections are present but wrong.
"""
import re
import sys

from checklib import ROOT, report

SECTIONS = ["Owns", "Depends on", "Entry points"]
LINTS = re.compile(r"^\[lints\]\s*\nworkspace\s*=\s*true", re.M)


def main():
    hits = []
    for crate in sorted(p for p in ROOT.glob("crates/*") if p.is_dir()):
        rel = crate.relative_to(ROOT).as_posix()
        for needed in ["Cargo.toml", "src/lib.rs", "README.md", "AGENTS.md"]:
            if not (crate / needed).is_file():
                hits.append(f"{rel}: missing {needed}")
        manifest = crate / "Cargo.toml"
        if manifest.is_file() and not LINTS.search(manifest.read_text(encoding="utf-8")):
            hits.append(f"{rel}/Cargo.toml: needs '[lints]' followed by 'workspace = true'")
        readme = crate / "README.md"
        if not readme.is_file():
            continue
        text = readme.read_text(encoding="utf-8")
        for sec in SECTIONS:
            m = re.search(rf"^## {re.escape(sec)}[ \t]*\n(.*?)(?=^## |\Z)", text, re.M | re.S)
            if not m:
                hits.append(f"{rel}/README.md: missing section '## {sec}'")
            elif not m.group(1).strip():
                hits.append(f"{rel}/README.md: section '## {sec}' is empty")
    return report("check-crate-layout", hits)


if __name__ == "__main__":
    sys.exit(main())
