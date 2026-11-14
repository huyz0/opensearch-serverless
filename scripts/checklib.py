"""Helpers shared by the check-*.py scripts.

Every check reads the tree under ROOT, which is the repository root unless
CHECK_ROOT is set. scripts/selftest-checks.py sets it to a temporary tree with
a planted violation to prove each check can fail.
"""
import os
import pathlib
import re
import subprocess

ROOT = pathlib.Path(os.environ.get("CHECK_ROOT") or pathlib.Path(__file__).resolve().parent.parent)


def tracked(suffixes=None):
    """Tracked and untracked-but-not-ignored files, relative to ROOT."""
    out = subprocess.run(
        ["git", "ls-files", "--cached", "--others", "--exclude-standard"],
        cwd=ROOT, capture_output=True, text=True, check=True,
    ).stdout
    for line in out.splitlines():
        if not (ROOT / line).is_file():
            continue
        if suffixes is None or pathlib.Path(line).suffix in suffixes:
            yield line


def read(rel):
    """The text of a file relative to ROOT."""
    return (ROOT / rel).read_text(encoding="utf-8")


def window_has(lines, i, marker, above=3):
    """True when `marker` is on line i or one of the `above` lines before it."""
    return any(marker in w for w in lines[max(0, i - above): i + 1])


TEST_ATTR = re.compile(r"#\[(?:[a-z_]+::)?test\b|proptest!")
FN_NAME = re.compile(r"\bfn\s+([a-z_][a-z0-9_]*)")


def test_names():
    """Names of every `#[test]` function (and proptest fn) under crates/."""
    names = set()
    for rel in tracked({".rs"}):
        if not rel.startswith("crates/"):
            continue
        lines = read(rel).splitlines()
        for i, line in enumerate(lines):
            if TEST_ATTR.search(line):
                for nxt in lines[i: i + 8]:
                    m = FN_NAME.search(nxt)
                    if m:
                        names.add(m.group(1))
                        break
    return names


EVIDENCE = re.compile(r"`([^`]+)`")


def unresolved_evidence(text, tests):
    """Backticked evidence items in `text` that do not resolve.

    An item resolves when it is an existing path (`scripts/check-x.py`), a test
    function name, or a `module::path::test_name` ending in one. Returns
    (resolved_count, unresolved_items).
    """
    ok, bad = 0, []
    for item in EVIDENCE.findall(text):
        path_part = item.split()[0]
        if (ROOT / path_part).exists() and ("/" in path_part or "." in path_part):
            ok += 1
        elif re.fullmatch(r"[a-z_][a-z0-9_]*(?:::[a-z_][a-z0-9_]*)*", item):
            if item.split("::")[-1] in tests:
                ok += 1
            else:
                bad.append(item)
    return ok, bad


def report(name, hits):
    """Print hits and return the process exit code."""
    if hits:
        print("\n".join(hits))
        print(f"{name}: {len(hits)} problem(s)")
        return 1
    print(f"{name}: ok")
    return 0
