#!/usr/bin/env python3
"""Proves every check script can fail.

For each check: build a small clean tree in a temporary git repository, show
the check passes on it, then plant one violation and show the check fails.
A check that passes on its planted violation is broken, and this fails.
Run by scripts/gate.sh; add a case whenever a check is added.
"""
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile

HERE = pathlib.Path(__file__).resolve().parent

CLEAN_BODY = "## Intent\nx\n## Alternatives rejected\nx\n## Invariants\nx\n## Evidence\nx\n## Risk tier\nlow\n## Where to look\nx\n"

CLEAN = {
    "Cargo.toml": '[workspace]\nmembers = ["crates/types"]\n\n[workspace.lints.rust]\nunsafe_code = "deny"\n',
    "crates/types/Cargo.toml": '[package]\nname = "oss-types"\n\n[dependencies]\n\n[dev-dependencies]\noss-testkit = { workspace = true }\n\n[lints]\nworkspace = true\n',
    "crates/types/README.md": "# oss-types\n\n## Owns\nIds.\n\n## Depends on\nNothing.\n\n## Entry points\n`src/lib.rs`.\n",
    "crates/types/AGENTS.md": "# notes\n",
    "crates/types/CLAUDE.md": "@AGENTS.md\n",
    "crates/types/src/lib.rs": "//! Ids.\n\n#[test]\nfn ids_are_distinct() {}\n\n#[allow(unsafe_code)]\nfn first(b: &[u8]) -> u8 {\n    // SAFETY: the caller checked b is not empty.\n    unsafe { *b.get_unchecked(0) }\n}\n",
    "crates/reader/Cargo.toml": '[package]\nname = "oss-reader"\n\n[dependencies]\noss-types = { workspace = true }\n\n[lints]\nworkspace = true\n',
    "crates/reader/README.md": "# oss-reader\n\n## Owns\nSearch.\n\n## Depends on\noss-types.\n\n## Entry points\n`src/lib.rs`.\n",
    "crates/reader/AGENTS.md": "# notes\n",
    "crates/reader/CLAUDE.md": "@AGENTS.md\n",
    "AGENTS.md": "Read the `demo` skill.\n",
    ".agents/skills/demo/SKILL.md": "---\nname: demo\ndescription: \"WHAT: A demo. USE WHEN: testing the checks.\"\n---\n\n# Demo\n\n## Enforced by\n\n- `scripts/check-demo.py`.\n",
    ".claude/skills": "../.agents/skills\n",
    "docs/mechanical-gates.md": "| `check-demo.py` | demo |\n",
    "scripts/tree-checks.sh": "check-demo.py\n",
    "scripts/check-demo.py": "",
    "msg.txt": "docs(x): explain a thing\n",
    "title.txt": "docs(x): explain a thing\n",
    "crates/reader/src/lib.rs": "//! Search.\n",
    "README.md": "See [types](crates/types/README.md).\n",
    "docs/invariants.md": "| Id | Invariant | Evidence |\n|---|---|---|\n| I-1 | ids differ | `ids_are_distinct` |\n| I-2 | later | NOT-YET: no code |\n",
    "docs/specs/ids/SPEC.md": "# Ids\n\n## Acceptance criteria\n\n- AC-1: ids differ\n",
    "docs/specs/ids/VERIFIED.md": "- AC-1: `ids_are_distinct`\n",
    "scripts/risk-paths.txt": "crates/blob/src/**  S3 request pattern\n",
    "scripts/size-baseline.txt": "",
    "crates/types/port-map.tsv": "rust_path\tkind\tjava_source\tnote\nsrc/lib.rs\tnew\t\tcrate root\n",
    "crates/reader/port-map.tsv": "rust_path\tkind\tjava_source\tnote\nsrc/lib.rs\tport\tserverless/Reader.java@abc\tquery phase\n",
    "body.md": CLEAN_BODY,
    "changed.txt": "crates/reader/src/lib.rs\n",
    "added.txt": "",
}

MSG = ["check-commit-msg.sh", "msg.txt"]
MSG_CI = ["check-commit-msg.sh", "--ci", "msg.txt"]
PR = ["check-pr.py", "--title", "title.txt", "--body", "body.md", "--changed", "changed.txt", "--added", "added.txt"]

# (check argv, description, files to overwrite or add; None deletes)
CASES = [
    (["check-layering.py"], "reader depends on a same-layer crate",
     {"crates/reader/Cargo.toml": '[package]\nname = "oss-reader"\n\n[dependencies]\noss-writer = { workspace = true }\n\n[lints]\nworkspace = true\n',
      "crates/writer/Cargo.toml": '[package]\nname = "oss-writer"\n\n[lints]\nworkspace = true\n'}),
    (["check-layering.py"], "test-only crate as a normal dependency",
     {"crates/reader/Cargo.toml": '[package]\nname = "oss-reader"\n\n[dependencies]\noss-testkit = { workspace = true }\n\n[lints]\nworkspace = true\n',
      "crates/testkit/Cargo.toml": '[package]\nname = "oss-testkit"\n'}),
    (["check-layering.py"], "same-layer crate that is not test-only, as a dev-dependency",
     {"crates/types/Cargo.toml": '[package]\nname = "oss-types"\n\n[dev-dependencies]\noss-mem = { workspace = true }\n\n[lints]\nworkspace = true\n'}),
    (["check-layering.py"], "standalone crate's bench names a workspace crate",
     {"crates/chunkjson/bench/Cargo.toml": '[package]\nname = "b"\n\n[dependencies]\nx = { path = "../../rest" }\n'}),
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
    (["check-budgets.py"], "an enforced row whose test file is missing",
     {"docs/request-budgets.md": "| Objective | Case | Budget | Bound | Status | Gate |\n|---|---|---|---|---|---|\n| `a.b` | x | 1 | exact | enforced | `crates/reader/tests/b.rs::gate` |\n"}),
    (["check-budgets.py"], "an enforced row whose test is missing",
     {"docs/request-budgets.md": "| Objective | Case | Budget | Bound | Status | Gate |\n|---|---|---|---|---|---|\n| `a.b` | x | 1 | exact | enforced | `crates/reader/tests/b.rs::gate` |\n",
      "crates/reader/tests/b.rs": "#[test]\nfn other() { check(\"a.b\", 1, \"\"); }\n"}),
    (["check-budgets.py"], "an enforced row whose test is ignored",
     {"docs/request-budgets.md": "| Objective | Case | Budget | Bound | Status | Gate |\n|---|---|---|---|---|---|\n| `a.b` | x | 1 | exact | enforced | `crates/reader/tests/b.rs::gate` |\n",
      "crates/reader/tests/b.rs": "#[test]\n#[ignore]\nfn gate() { check(\"a.b\", 1, \"\"); }\n"}),
    (["check-budgets.py"], "an enforced row whose function is no test",
     {"docs/request-budgets.md": "| Objective | Case | Budget | Bound | Status | Gate |\n|---|---|---|---|---|---|\n| `a.b` | x | 1 | exact | enforced | `crates/reader/tests/b.rs::gate` |\n",
      "crates/reader/tests/b.rs": "fn gate() { check(\"a.b\", 1, \"\"); }\n"}),
    (["check-budgets.py"], "an enforced row whose key is outside its test's body",
     {"docs/request-budgets.md": "| Objective | Case | Budget | Bound | Status | Gate |\n|---|---|---|---|---|---|\n| `a.b` | x | 1 | exact | enforced | `crates/reader/tests/b.rs::gate` |\n",
      "crates/reader/tests/b.rs": "#[test]\nfn gate() { check(\"a.c\", 1, \"\"); }\n// \"a.b\"\n"}),
    (["check-budgets.py"], "an enforced row whose test module no target declares",
     {"docs/request-budgets.md": "| Objective | Case | Budget | Bound | Status | Gate |\n|---|---|---|---|---|---|\n| `a.b` | x | 1 | exact | enforced | `crates/reader/tests/it/b.rs::gate` |\n",
      "crates/reader/tests/it/main.rs": "mod other;\n",
      "crates/reader/tests/it/b.rs": "#[test]\nfn gate() { check(\"a.b\", 1, \"\"); }\n"}),
    (["check-budgets.py"], "an enforced row whose test never names its key",
     {"docs/request-budgets.md": "| Objective | Case | Budget | Bound | Status | Gate |\n|---|---|---|---|---|---|\n| `a.b` | x | 1 | exact | enforced | `crates/reader/tests/b.rs::gate` |\n",
      "crates/reader/tests/b.rs": "#[test]\nfn gate() { check(\"a.c\", 1, \"\"); }\n"}),
    (["check-budgets.py"], "a blocked row naming no blocker",
     {"docs/request-budgets.md": "| Objective | Case | Budget | Bound | Status | Gate |\n|---|---|---|---|---|---|\n| `a.b` | x | 0 | exact | blocked |  |\n"}),
    (["check-budgets.py"], "a row with a third status",
     {"docs/request-budgets.md": "| Objective | Case | Budget | Bound | Status | Gate |\n|---|---|---|---|---|---|\n| `a.b` | x | 0 | exact | aspirational | soon |\n"}),
    (["check-budgets.py"], "an objective twice",
     {"docs/request-budgets.md": "| Objective | Case | Budget | Bound | Status | Gate |\n|---|---|---|---|---|---|\n| `a.b` | x | 0 | exact | blocked | y |\n| `a.b` | x | 0 | max | blocked | y |\n"}),
    (["check-test-endpoints.py"], "real AWS endpoint", {"crates/reader/src/lib.rs": '//! S.\nconst E: &str = "https://s3.us-east-1.amazonaws.com";\n'}),
    (["check-alloc.py"], "request-path Vec::new without a bound",
     {"crates/reader/src/lib.rs": "//! S.\nfn f() -> Vec<u8> { Vec::new() }\n"}),
    (["check-alloc.py"], "unbounded channel even with a comment",
     {"crates/reader/src/lib.rs": "//! S.\n// ALLOC: fine\nfn f() { let _ = std::sync::mpsc::channel::<u8>(); }\n"}),
    (["check-arith-allows.py"], "allow without proof",
     {"crates/reader/src/lib.rs": "//! S.\n#[allow(clippy::arithmetic_side_effects)]\nfn f(a: u8) -> u8 { a + 1 }\n"}),
    (["check-dev-notes.py"], "milestone id in a comment", {"crates/reader/src/lib.rs": "//! S.\n// added in M3\n"}),
    (["check-links.py"], "broken relative link", {"README.md": "See [gone](missing.md).\n"}),
    (["check-unsafe.py"], "crate-wide allow(unsafe_code)", {"crates/reader/src/lib.rs": "//! S.\n#![allow(unsafe_code)]\n"}),
    (["check-unsafe.py"], "item-level warn(unsafe_code)", {"crates/reader/src/lib.rs": "//! S.\n#[warn(unsafe_code)]\nfn f() {}\n"}),
    (["check-unsafe.py"], "unsafe impl Send", {"crates/reader/src/lib.rs": "//! S.\nstruct P;\nunsafe impl Send for P {}\n"}),
    (["check-unsafe.py"], "transmute", {"crates/reader/src/lib.rs": "//! S.\nfn f(x: u32) -> f32 { unsafe { std::mem::transmute(x) } }\n"}),
    (["check-unsafe.py"], "extern block", {"crates/reader/src/lib.rs": '//! S.\nunsafe extern "C" { fn abs(x: i32) -> i32; }\n'}),
    (["check-unsafe.py"], "unsafe_code relaxed in a manifest",
     {"Cargo.toml": '[workspace]\n\n[workspace.lints.rust]\nunsafe_code = "allow"\n'}),
    (["check-invariants.py"], "invariant naming a missing test",
     {"docs/invariants.md": "| Id | Invariant | Evidence |\n|---|---|---|\n| I-1 | ids differ | `no_such_test` |\n"}),
    (["check-invariants.py"], "invariant with no evidence",
     {"docs/invariants.md": "| Id | Invariant | Evidence |\n|---|---|---|\n| I-1 | ids differ | trust me |\n"}),
    (["check-verified.py"], "criterion without evidence",
     {"docs/specs/ids/SPEC.md": "# Ids\n\n## Acceptance criteria\n\n- AC-1: ids differ\n- AC-2: ids sort\n"}),
    (["check-verified.py"], "evidence naming a missing test", {"docs/specs/ids/VERIFIED.md": "- AC-1: `no_such_test`\n"}),
    (["check-port-map.py"], "Rust file without a row", {"crates/types/src/ids.rs": "//! x\n"}),
    (["check-port-map.py"], "port row without its Java source",
     {"crates/reader/port-map.tsv": "rust_path\tkind\tjava_source\tnote\nsrc/lib.rs\tport\t\tquery phase\n"}),
    (["check-crate-layout.py"], "crate without its CLAUDE.md adapter", {"crates/types/CLAUDE.md": None}),
    (["check-harness.py"], "no .claude/skills link", {".claude/skills": None}),
    (["check-harness.py"], "skill name differs from its directory",
     {".agents/skills/demo/SKILL.md": "---\nname: other\ndescription: \"WHAT: A demo. USE WHEN: testing.\"\n---\n\n## Enforced by\n\n- x\n"}),
    (["check-harness.py"], "skill description without USE WHEN",
     {".agents/skills/demo/SKILL.md": "---\nname: demo\ndescription: A demo skill\n---\n\n## Enforced by\n\n- x\n"}),
    (["check-harness.py"], "skill without Enforced by",
     {".agents/skills/demo/SKILL.md": "---\nname: demo\ndescription: \"WHAT: A demo. USE WHEN: testing.\"\n---\n\n# Demo\n"}),
    (["check-harness.py"], "skill over 100 lines",
     {".agents/skills/demo/SKILL.md": "---\nname: demo\ndescription: \"WHAT: A demo. USE WHEN: testing.\"\n---\n\n## Enforced by\n\n- x\n" + "line\n" * 100}),
    (["check-harness.py"], "reference to a missing skill", {"AGENTS.md": "Read the `gone` skill.\n"}),
    (["check-harness.py"], "reference to a missing script", {"AGENTS.md": "Run `scripts/gone.sh`.\n"}),
    (["check-harness.py"], "AGENTS.md over its budget", {"AGENTS.md": "x" * 6000}),
    (["check-harness.py"], "tree check with no mechanical-gates row", {"docs/mechanical-gates.md": ""}),
    (["check-extension-pins.py"], "a shipped configuration turning pins on",
     {"deploy/node.yml": "serverless.extensions.pins.allowed: true\n"}),
    (["check-extension-pins.py"], "a shell script turning pins on",
     {"scripts/run.sh": "export X='\"serverless.extensions.pins.allowed\": true'\n"}),
    (["check-harness.py"], "check script nothing runs", {"scripts/check-orphan.py": ""}),
    (["check-harness.py"], "check script named only in a comment",
     {"scripts/check-orphan.py": "", "scripts/gate.sh": "# check-orphan.py is run elsewhere\n"}),
    (MSG, "Co-Authored-By naming Claude",
     {"msg.txt": "docs(x): explain a thing\n\nCo-Authored-By: Claude <noreply@anthropic.com>\n"}),
    (MSG, "Claude-Session trailer", {"msg.txt": "docs(x): explain a thing\n\nClaude-Session: https://claude.ai/code/session_1\n"}),
    (MSG, "Revert commit carrying Co-Authored-By: Claude",
     {"msg.txt": "Revert \"docs(x): explain\"\n\nCo-Authored-By: Claude <noreply@anthropic.com>\n"}),
    (MSG, "Generated-with footer", {"msg.txt": "docs(x): explain a thing\n\nGenerated with [Claude Code](https://claude.com/claude-code)\n"}),
    (MSG, "subject not in type(scope) form", {"msg.txt": "Explain a thing\n"}),
    (MSG, "fix commit without Spec", {"msg.txt": "fix(x): repair a thing\n"}),
    (MSG_CI, "ci mode still refuses attribution",
     {"msg.txt": "fix(x): repair a thing\n\nCo-Authored-By: Claude <noreply@anthropic.com>\n"}),
    (PR, "PR title not in type(scope) form", {"title.txt": "Explain a thing\n"}),
    (PR, "fix PR without a Spec line", {"title.txt": "fix(x): repair a thing\n"}),
    (PR, "fix PR naming a missing spec",
     {"title.txt": "fix(x): repair a thing\n", "body.md": CLEAN_BODY + "\nSpec: docs/specs/gone/SPEC.md\n"}),
    (PR, "PR body with an attribution footer",
     {"body.md": CLEAN_BODY + "\nGenerated with [Claude Code](https://claude.com/claude-code)\n"}),
    (PR, "PR body with a session link", {"body.md": CLEAN_BODY + "\nhttps://claude.ai/code/session_01abc\n"}),
    (PR, "PR body with an attribution header", {"body.md": "<!-- claude-projects-attribution: {} -->\n" + CLEAN_BODY}),
    (PR, "PR body with a Requested-by line", {"body.md": "_Requested by **Huy**_\n" + CLEAN_BODY}),
    (PR, "PR body missing a section", {"body.md": "## Intent\nx\n"}),
    (PR, "new workspace crate without an ADR",
     {"changed.txt": "crates/extra/Cargo.toml\n", "added.txt": "crates/extra/Cargo.toml\n"}),
    (PR, "high-risk path without label or ADR", {"changed.txt": "crates/blob/src/store.rs\n"}),
]

# (argv, description, overrides) that must pass: a rule moved, not dropped.
OK_CASES = [
    (["check-extension-pins.py"], "pins left off in a configuration",
     {"deploy/node.yml": "serverless.extensions.pins.allowed: false\n"}),
    (MSG_CI, "ci mode accepts a fix with no Spec and a free-form subject", {"msg.txt": "Repair a thing\n"}),
    (PR, "docs PR title needs no Spec", {}),
    (PR, "fix PR with Spec: none", {"title.txt": "fix(x): repair a thing\n", "body.md": CLEAN_BODY + "\nSpec: none (typo)\n"}),
    (PR, "fix PR naming an existing spec",
     {"title.txt": "fix(x): repair a thing\n", "body.md": CLEAN_BODY + "\nSpec: docs/specs/ids/SPEC.md\n"}),
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
    # shutil.which searches PATH, so Windows finds Git Bash, not the WSL launcher.
    runner = [shutil.which("bash") or "bash"] if argv[0].endswith(".sh") else [sys.executable]
    args = runner + [str(HERE / argv[0])] + [str(tmp / a) if a.endswith((".md", ".txt")) else a for a in argv[1:]]
    return subprocess.run(args, cwd=tmp, env=env, capture_output=True, text=True).returncode


def main():
    failures = []
    checks = sorted({c[0][0] for c in CASES})
    with tempfile.TemporaryDirectory() as d:
        tmp = pathlib.Path(d)
        build(tmp, {})
        for check in checks:
            argv = {"check-pr.py": PR, "check-commit-msg.sh": MSG}.get(check, [check])
            if run(tmp, argv) != 0:
                failures.append(f"{check}: fails on the clean tree")
    for argv, desc, overrides in CASES:
        with tempfile.TemporaryDirectory() as d:
            tmp = pathlib.Path(d)
            build(tmp, overrides)
            if run(tmp, argv) == 0:
                failures.append(f"{argv[0]}: passed on planted violation: {desc}")
    for argv, desc, overrides in OK_CASES:
        with tempfile.TemporaryDirectory() as d:
            tmp = pathlib.Path(d)
            build(tmp, overrides)
            if run(tmp, argv) != 0:
                failures.append(f"{argv[0]}: failed a case that must pass: {desc}")
    if failures:
        print("\n".join(failures))
        print(f"selftest-checks: {len(failures)} problem(s)")
        return 1
    print(f"selftest-checks: ok ({len(checks)} checks pass clean, {len(CASES)} planted violations caught, {len(OK_CASES)} allowed cases pass)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
