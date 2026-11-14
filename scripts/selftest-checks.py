#!/usr/bin/env python3
"""Proves every check script can fail.

For each check: build a small clean tree in a temporary git repository, show
the check passes on it, then plant one violation and show the check fails.
A check that passes on its planted violation is broken, and this fails.
Run by scripts/gate.sh; add a case whenever a check is added.
"""
import os
import pathlib
import subprocess
import sys
import tempfile

HERE = pathlib.Path(__file__).resolve().parent

CLEAN = {
    "Cargo.toml": '[workspace]\nmembers = ["crates/types"]\n\n[workspace.lints.rust]\nunsafe_code = "forbid"\n',
    "crates/types/Cargo.toml": '[package]\nname = "oss-types"\n\n[dependencies]\n\n[lints]\nworkspace = true\n',
    "crates/types/README.md": "# oss-types\n\n## Owns\nIds.\n\n## Depends on\nNothing.\n\n## Entry points\n`src/lib.rs`.\n",
    "crates/types/AGENTS.md": "# notes\n",
    "crates/types/src/lib.rs": "//! Ids.\n\n#[test]\nfn ids_are_distinct() {}\n",
    "crates/reader/Cargo.toml": '[package]\nname = "oss-reader"\n\n[dependencies]\noss-types = { workspace = true }\n\n[lints]\nworkspace = true\n',
    "crates/reader/README.md": "# oss-reader\n\n## Owns\nSearch.\n\n## Depends on\noss-types.\n\n## Entry points\n`src/lib.rs`.\n",
    "crates/reader/AGENTS.md": "# notes\n",
    "crates/reader/src/lib.rs": "//! Search.\n",
    "README.md": "See [types](crates/types/README.md).\n",
    "docs/invariants.md": "| Id | Invariant | Evidence |\n|---|---|---|\n| I-1 | ids differ | `ids_are_distinct` |\n| I-2 | later | NOT-YET: no code |\n",
    "docs/specs/ids/SPEC.md": "# Ids\n\n## Acceptance criteria\n\n- AC-1: ids differ\n",
    "docs/specs/ids/VERIFIED.md": "- AC-1: `ids_are_distinct`\n",
    "scripts/risk-paths.txt": "crates/blob/src/**  S3 request pattern\n",
    "scripts/size-baseline.txt": "",
    "docs/port-map.tsv": "rust_path\tkind\tjava_source\tnote\ncrates/types/src/lib.rs\tnew\t\tcrate root\ncrates/reader/src/lib.rs\tport\tserverless/Reader.java\tquery phase\n",
    "body.md": "## Intent\nx\n## Alternatives rejected\nx\n## Invariants\nx\n## Evidence\nx\n## Risk tier\nlow\n## Where to look\nx\n",
    "changed.txt": "crates/reader/src/lib.rs\n",
    "added.txt": "",
}

PR = ["check-pr.py", "--body", "body.md", "--changed", "changed.txt", "--added", "added.txt"]

# (check argv, description, files to overwrite or add; None deletes)
CASES = [
    (["check-layering.py"], "reader depends on a same-layer crate",
     {"crates/reader/Cargo.toml": '[package]\nname = "oss-reader"\n\n[dependencies]\noss-writer = { workspace = true }\n\n[lints]\nworkspace = true\n',
      "crates/writer/Cargo.toml": '[package]\nname = "oss-writer"\n\n[lints]\nworkspace = true\n'}),
    (["check-layering.py"], "crate with no layer",
     {"crates/extra/Cargo.toml": '[package]\nname = "oss-extra"\n'}),
    (["check-crate-layout.py"], "README without Entry points",
     {"crates/types/README.md": "# oss-types\n\n## Owns\nIds.\n\n## Depends on\nNothing.\n"}),
    (["check-crate-layout.py"], "crate without AGENTS.md", {"crates/types/AGENTS.md": None}),
    (["check-crate-layout.py"], "crate opting out of workspace lints",
     {"crates/types/Cargo.toml": '[package]\nname = "oss-types"\n'}),
    (["check-module-size.py"], "1,001-line file", {"crates/types/src/big.rs": "// x\n" * 1001}),
    (["check-module-size.py"], "501-line file not on the baseline", {"crates/types/src/big.rs": "// x\n" * 501}),
    (["check-module-size.py"], "baselined file grew",
     {"crates/types/src/big.rs": "// x\n" * 520, "scripts/size-baseline.txt": "crates/types/src/big.rs 510\n"}),
    (["check-s3-access.py"], "S3 client outside oss-blob",
     {"crates/reader/Cargo.toml": '[package]\nname = "oss-reader"\n\n[dependencies]\naws-sdk-s3 = "1"\n\n[lints]\nworkspace = true\n'}),
    (["check-s3-access.py"], "unjustified LIST", {"crates/reader/src/lib.rs": "//! Search.\nfn f(c: &C) { c.list_objects_v2(); }\n"}),
    (["check-test-endpoints.py"], "real AWS endpoint", {"crates/reader/src/lib.rs": '//! S.\nconst E: &str = "https://s3.us-east-1.amazonaws.com";\n'}),
    (["check-alloc.py"], "request-path Vec::new without a bound",
     {"crates/reader/src/lib.rs": "//! S.\nfn f() -> Vec<u8> { Vec::new() }\n"}),
    (["check-alloc.py"], "unbounded channel even with a comment",
     {"crates/reader/src/lib.rs": "//! S.\n// ALLOC: fine\nfn f() { let _ = std::sync::mpsc::channel::<u8>(); }\n"}),
    (["check-arith-allows.py"], "allow without proof",
     {"crates/reader/src/lib.rs": "//! S.\n#[allow(clippy::arithmetic_side_effects)]\nfn f(a: u8) -> u8 { a + 1 }\n"}),
    (["check-dev-notes.py"], "milestone id in a comment", {"crates/reader/src/lib.rs": "//! S.\n// added in M3\n"}),
    (["check-links.py"], "broken relative link", {"README.md": "See [gone](missing.md).\n"}),
    (["check-unsafe.py"], "allow(unsafe_code) in Rust", {"crates/reader/src/lib.rs": "//! S.\n#![allow(unsafe_code)]\n"}),
    (["check-unsafe.py"], "unsafe_code relaxed in a manifest",
     {"Cargo.toml": '[workspace]\n\n[workspace.lints.rust]\nunsafe_code = "warn"\n'}),
    (["check-invariants.py"], "invariant naming a missing test",
     {"docs/invariants.md": "| Id | Invariant | Evidence |\n|---|---|---|\n| I-1 | ids differ | `no_such_test` |\n"}),
    (["check-invariants.py"], "invariant with no evidence",
     {"docs/invariants.md": "| Id | Invariant | Evidence |\n|---|---|---|\n| I-1 | ids differ | trust me |\n"}),
    (["check-verified.py"], "criterion without evidence",
     {"docs/specs/ids/SPEC.md": "# Ids\n\n## Acceptance criteria\n\n- AC-1: ids differ\n- AC-2: ids sort\n"}),
    (["check-verified.py"], "evidence naming a missing test", {"docs/specs/ids/VERIFIED.md": "- AC-1: `no_such_test`\n"}),
    (["check-port-map.py"], "Rust file without a row", {"crates/types/src/ids.rs": "//! x\n"}),
    (["check-port-map.py"], "port row without its Java source",
     {"docs/port-map.tsv": "rust_path\tkind\tjava_source\tnote\ncrates/types/src/lib.rs\tnew\t\tcrate root\ncrates/reader/src/lib.rs\tport\t\tquery phase\n"}),
    (PR, "PR body missing a section", {"body.md": "## Intent\nx\n"}),
    (PR, "high-risk path without label or ADR", {"changed.txt": "crates/blob/src/store.rs\n"}),
]


def build(tmp, overrides):
    files = dict(CLEAN)
    files.update(overrides)
    for rel, text in files.items():
        if text is None:
            continue
        path = tmp / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8", newline="\n")
    subprocess.run(["git", "init", "-q"], cwd=tmp, check=True)


def run(tmp, argv):
    env = dict(os.environ, CHECK_ROOT=str(tmp))
    args = [sys.executable, str(HERE / argv[0])] + [str(tmp / a) if a.endswith((".md", ".txt")) else a for a in argv[1:]]
    return subprocess.run(args, cwd=tmp, env=env, capture_output=True, text=True).returncode


def main():
    failures = []
    checks = sorted({c[0][0] for c in CASES})
    with tempfile.TemporaryDirectory() as d:
        tmp = pathlib.Path(d)
        build(tmp, {})
        for check in checks:
            argv = PR if check == "check-pr.py" else [check]
            if run(tmp, argv) != 0:
                failures.append(f"{check}: fails on the clean tree")
    for argv, desc, overrides in CASES:
        with tempfile.TemporaryDirectory() as d:
            tmp = pathlib.Path(d)
            build(tmp, overrides)
            if run(tmp, argv) == 0:
                failures.append(f"{argv[0]}: passed on planted violation: {desc}")
    if failures:
        print("\n".join(failures))
        print(f"selftest-checks: {len(failures)} problem(s)")
        return 1
    print(f"selftest-checks: ok ({len(checks)} checks pass clean, {len(CASES)} planted violations caught)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
