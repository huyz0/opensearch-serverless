#!/usr/bin/env python3
"""Every row of docs/request-budgets.md is enforced by a gate or blocked.

A row is `enforced` with a gate `crates/<crate>/tests/<path>.rs::<fn>`, or
`blocked` with its blocker named; there is no third status. Fails on a
malformed row (the table's own parser in `oss_testkit::budget` would refuse
it too); an enforced row whose test file does not exist or is no test
target (a file directly under `tests/`, or `tests/<dir>/main.rs`, or a
module `tests/<dir>/main.rs` declares); whose `fn <fn>` is missing, carries
no `#[test]`, or is `#[ignore]`d; or whose function body never names the
row's key as a string; and a blocked row whose gate cell is empty.

Cannot catch: a gate that names the key but measures something else, or a
module declared behind a `cfg` that is off. Review covers what a gate
measures.
"""
import re
import sys

from checklib import ROOT, read, report

TABLE = "docs/request-budgets.md"
KEY = re.compile(r"`([a-z0-9_.]+)`")
GATE = re.compile(r"(crates/[A-Za-z0-9_./-]+/tests/[A-Za-z0-9_./-]+\.rs)::([a-z_][a-z0-9_]*)")


def rows(text, problems):
    """The table's rows as (line, key, status, gate)."""
    seen = set()
    for n, raw in enumerate(text.splitlines(), 1):
        line = raw.strip()
        if not line.startswith("|") or line.startswith("|---") or line.startswith("| Objective"):
            continue
        cells = [c.strip() for c in line.strip("|").split("|")]
        where = f"{TABLE}:{n}"
        if len(cells) != 6:
            problems.append(f"{where}: expected 6 cells, found {len(cells)}")
            continue
        key = KEY.fullmatch(cells[0])
        if not key:
            problems.append(f"{where}: the objective is not a `lower.case_key`: {cells[0]}")
            continue
        if not cells[2].replace(",", "").isdigit() or cells[3] not in ("exact", "max") \
                or cells[4] not in ("enforced", "blocked"):
            problems.append(f"{where}: budget must be a number, bound exact or max, status enforced or blocked")
            continue
        if key.group(1) in seen:
            problems.append(f"{where}: objective {key.group(1)} appears twice")
            continue
        seen.add(key.group(1))
        yield n, key.group(1), cells[4], cells[5].replace("`", "").strip()


def target_problem(path):
    """Why `path` is not compiled into a test binary, or None."""
    crate, rest = path.split("/tests/", 1)
    parts = rest.split("/")
    if len(parts) == 1:
        return None
    if len(parts) == 2:
        if parts[1] == "main.rs":
            return None
        main = f"{crate}/tests/{parts[0]}/main.rs"
        stem = parts[1][:-3]
        if (ROOT / main).is_file() and re.search(rf"^\s*(pub(\(crate\))?\s+)?mod\s+{stem}\s*;", read(main), re.M):
            return None
        return f"{path} is declared by no test target ({main} has no `mod {stem};`)"
    return f"{path} is nested too deep to be a test target this check can follow"


def gate_problem(source, name, key):
    """Why `fn name` in `source` does not check `key`, or None."""
    lines = source.splitlines()
    for i, line in enumerate(lines):
        if not re.search(rf"\bfn\s+{name}\s*\(", line):
            continue
        attrs = []
        j = i - 1
        while j >= 0 and (lines[j].strip().startswith("#[") or lines[j].strip().startswith("//")):
            attrs.append(lines[j].strip())
            j -= 1
        if not any(re.match(r"#\[(?:[a-z_]+::)?test\]", a) for a in attrs):
            return f"has fn {name}, but it is no #[test]"
        if any(a.startswith("#[ignore") for a in attrs):
            return f"has fn {name}, but it is #[ignore]d"
        body = function_body("\n".join(lines[i:]))
        if f'"{key}"' not in body:
            return f"fn {name} never names the key in its body, so it cannot check it"
        return None
    return f"defines no fn {name}"


def function_body(text):
    """The text from a function's opening brace to its matching close."""
    start = text.find("{")
    if start < 0:
        return ""
    depth = 0
    for k in range(start, len(text)):
        if text[k] == "{":
            depth += 1
        elif text[k] == "}":
            depth -= 1
            if depth == 0:
                return text[start:k + 1]
    return text[start:]


def main():
    if not (ROOT / TABLE).is_file():
        return report("check-budgets", [])
    problems = []
    for n, key, status, gate in list(rows(read(TABLE), problems)):
        where = f"{TABLE}:{n} {key}"
        if status == "blocked":
            if gate.lower() in ("", "-", "none"):
                problems.append(f"{where}: blocked, but names no blocker")
            continue
        m = GATE.fullmatch(gate)
        if not m:
            problems.append(f"{where}: enforced, but the gate is not crates/<crate>/tests/<path>.rs::<fn>: {gate}")
            continue
        path, name = m.groups()
        if not (ROOT / path).is_file():
            problems.append(f"{where}: enforced, but {path} does not exist")
            continue
        unreachable = target_problem(path)
        if unreachable:
            problems.append(f"{where}: enforced, but {unreachable}")
            continue
        problem = gate_problem(read(path), name, key)
        if problem:
            problems.append(f"{where}: enforced, but {path} {problem}")
    return report("check-budgets", problems)


if __name__ == "__main__":
    sys.exit(main())
