#!/usr/bin/env python3
"""Generate the crate map in docs/architecture.md from `cargo metadata`.

The block between `<!-- crate-map:start -->` and `<!-- crate-map:end -->` is
a Mermaid graph of workspace crates and their workspace dependencies (optional
ones dashed). Nobody edits it by hand.

  gen-crate-map.py          rewrite the block
  gen-crate-map.py --check  fail when the block is stale, or when
                            architecture.md exceeds 1,500 words

Cannot catch: a stale prose description elsewhere in the file.
"""
import json
import re
import subprocess
import sys

from checklib import ROOT

DOC = ROOT / "docs/architecture.md"
START, END = "<!-- crate-map:start -->", "<!-- crate-map:end -->"
MAX_WORDS = 1500


def render():
    meta = json.loads(subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        cwd=ROOT, capture_output=True, text=True, check=True,
    ).stdout)
    names = {p["name"] for p in meta["packages"]}
    lines = ["```mermaid", "graph TD"]
    for pkg in sorted(meta["packages"], key=lambda p: p["name"]):
        deps = sorted(
            {(d["name"], d.get("optional", False)) for d in pkg["dependencies"]
             if d["name"] in names and d.get("kind") in (None, "normal")}
        )
        if not deps:
            lines.append(f"    {pkg['name']}")
        for dep, optional in deps:
            arrow = "-.->" if optional else "-->"
            lines.append(f"    {pkg['name']} {arrow} {dep}")
    lines.append("```")
    return "\n".join(lines)


def main():
    text = DOC.read_text(encoding="utf-8")
    block = re.compile(re.escape(START) + r".*?" + re.escape(END), re.S)
    if not block.search(text):
        print(f"gen-crate-map: {DOC.name} has no {START} ... {END} block")
        return 1
    fresh = block.sub(lambda _: f"{START}\n{render()}\n{END}", text)
    if "--check" in sys.argv:
        problems = []
        if fresh != text:
            problems.append("crate map is stale: run scripts/gen-crate-map.py")
        words = len(re.sub(r"```.*?```", "", text, flags=re.S).split())
        if words > MAX_WORDS:
            problems.append(f"architecture.md is {words} words (limit {MAX_WORDS}); move detail to crate READMEs or ADRs")
        for p in problems:
            print(f"gen-crate-map: {p}")
        if not problems:
            print("gen-crate-map: ok")
        return 1 if problems else 0
    DOC.write_text(fresh, encoding="utf-8", newline="\n")
    print("gen-crate-map: written")
    return 0


if __name__ == "__main__":
    sys.exit(main())
