#!/usr/bin/env python3
"""Decides what a change needs CI to run.

  ci-changes.py FILE            changed paths, one per line
  ci-changes.py --self-test     prove the classification on fixed examples

Prints `key=value` lines (append them to $GITHUB_OUTPUT):
  full=true|false     run everything: Cargo.lock, the root manifest, the
                      toolchain, .cargo/, CI workflows or the gate changed
  rust=true|false     any Rust build or test is needed
  packages=<-p list>  the changed crates plus every workspace crate that
                      depends on them, as `-p a -p b` (empty when full)

Docs-only and script-only changes give rust=false: the tree checks still run.
Any path this script does not recognise answers "run everything", because a
wrong skip costs more than a wasted run.

Cannot catch: a crate that reads a non-Rust file outside its directory (none
do today; such a file belongs in FULL below).
"""
import json
import re
import subprocess
import sys

from checklib import ROOT

FULL = [r"^Cargo\.lock$", r"^Cargo\.toml$", r"^rust-toolchain\.toml$", r"^\.cargo/",
        r"^\.config/nextest\.toml$", r"^\.github/workflows/", r"^scripts/gate\.sh$", r"^scripts/coverage\.sh$"]
TREE_ONLY = [r"\.md$", r"^docs/", r"^\.agents/", r"^\.claude/", r"^scripts/", r"^\.githooks/",
             r"^\.github/pull_request_template\.md$", r"^LICENSE$", r"^\.git(ignore|attributes)$", r"^CLAUDE\.md$"]
CRATE = re.compile(r"^crates/([^/]+)/")


def workspace():
    """{crate dir name: package name} and {package: set of workspace deps}."""
    meta = json.loads(subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"],
        cwd=ROOT, capture_output=True, text=True, check=True).stdout)
    names = {p["name"] for p in meta["packages"]}
    by_dir, deps = {}, {}
    for p in meta["packages"]:
        d = re.search(r"crates[/\\]([^/\\]+)[/\\]Cargo\.toml$", p["manifest_path"])
        if d:
            by_dir[d.group(1)] = p["name"]
        deps[p["name"]] = {x["name"] for x in p["dependencies"] if x["name"] in names}
    return by_dir, deps


def classify(paths, by_dir, deps):
    full, crates = False, set()
    for path in paths:
        if any(re.search(p, path) for p in FULL):
            full = True
        elif (m := CRATE.match(path)) and m.group(1) in by_dir:
            if not path.endswith(".md"):
                crates.add(by_dir[m.group(1)])
        elif any(re.search(p, path) for p in TREE_ONLY):
            continue
        else:
            full = True
    affected = set(crates)
    grew = True
    while grew:
        grew = False
        for pkg, ds in deps.items():
            if pkg not in affected and ds & affected:
                affected.add(pkg)
                grew = True
    rust = full or bool(affected)
    packages = "" if full else " ".join(f"-p {p}" for p in sorted(affected))
    return {"full": str(full).lower(), "rust": str(rust).lower(), "packages": packages}


def self_test():
    by_dir = {"types": "oss-types", "blob": "oss-blob", "reader": "oss-reader", "node": "oss-node"}
    deps = {"oss-types": set(), "oss-blob": {"oss-types"}, "oss-reader": {"oss-blob"}, "oss-node": {"oss-reader"}}
    cases = [
        (["docs/ci.md", "README.md"], {"full": "false", "rust": "false", "packages": ""}),
        (["scripts/check-links.py"], {"full": "false", "rust": "false", "packages": ""}),
        (["crates/blob/README.md"], {"full": "false", "rust": "false", "packages": ""}),
        (["crates/reader/src/lib.rs"], {"full": "false", "rust": "true", "packages": "-p oss-node -p oss-reader"}),
        (["crates/types/src/lib.rs"], {"full": "false", "rust": "true",
                                       "packages": "-p oss-blob -p oss-node -p oss-reader -p oss-types"}),
        (["Cargo.lock"], {"full": "true", "rust": "true", "packages": ""}),
        (["something/new.bin"], {"full": "true", "rust": "true", "packages": ""}),
    ]
    bad = [f"{paths} -> {got}, expected {want}" for paths, want in cases
           if (got := classify(paths, by_dir, deps)) != want]
    print("\n".join(bad) if bad else f"ci-changes: self-test ok ({len(cases)} cases)")
    return 1 if bad else 0


def main():
    if "--self-test" in sys.argv:
        return self_test()
    with open(sys.argv[1], encoding="utf-8") as f:
        paths = [line.strip() for line in f if line.strip()]
    by_dir, deps = workspace()
    for k, v in classify(paths, by_dir, deps).items():
        print(f"{k}={v}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
