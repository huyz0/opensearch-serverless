#!/usr/bin/env python3
"""Every allocation on a request path names its bound.

In the request-path crates (REQUEST_PATH), each allocation site needs an
`// ALLOC:` comment on its line or within the three lines above, saying what
bounds it (for example `// ALLOC: one per request, at most MAX_SHARDS`).
Steady-state request paths should use pooled buffers and arenas instead
(docs/adr/0004-bounded-memory-and-streaming.md). Unbounded channels are
refused outright, with or without a comment. Code after `#[cfg(test)]` in a
file, and files under tests/, are exempt.

Cannot catch: an allocation hidden inside a called function or a dependency,
or a comment whose stated bound is false. Peak-memory tests cover those.
"""
import re
import sys

from checklib import read, report, tracked, window_has

REQUEST_PATH = (
    "crates/blob/src/", "crates/format/src/", "crates/rest/src/", "crates/edge/src/",
    "crates/meta/src/", "crates/cluster/src/", "crates/transport/src/", "crates/reader/src/", "crates/writer/src/", "crates/coordinator/src/",
)
ALLOC = re.compile(
    r"\bVec::new\s*\(|\bvec!\s*\[|\bString::new\s*\(|\bformat!\s*\(|\.to_vec\s*\(|\.to_string\s*\(|"
    r"\.to_owned\s*\(|\.collect\s*(::<[^>]*>)?\s*\(|\bBox::new\s*\(|::with_capacity\s*\(|\bHashMap::new\s*\(|"
    r"\bBTreeMap::new\s*\(|\bVecDeque::new\s*\("
)
TURBOFISH = r"\s*(?:::<[^>]*>)?\s*\("
UNBOUNDED = re.compile(rf"\bunbounded_channel{TURBOFISH}|\bmpsc::channel{TURBOFISH}|\bunbounded{TURBOFISH}")


def main():
    hits = []
    for rel in tracked({".rs"}):
        if not rel.startswith(REQUEST_PATH):
            continue
        lines = read(rel).splitlines()
        for i, line in enumerate(lines):
            if "#[cfg(test)]" in line:
                break
            code = line.split("//", 1)[0]
            if UNBOUNDED.search(code):
                hits.append(f"{rel}:{i + 1}: unbounded channel; use a bounded one")
            elif ALLOC.search(code) and not window_has(lines, i, "// ALLOC:"):
                hits.append(f"{rel}:{i + 1}: allocation without an '// ALLOC:' bound (or use a pooled buffer)")
    return report("check-alloc", hits)


if __name__ == "__main__":
    sys.exit(main())
