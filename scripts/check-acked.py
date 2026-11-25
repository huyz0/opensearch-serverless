#!/usr/bin/env python3
"""A read of a shard engine's highest sequence number says why it is safe.

With a write pipeline, the engine's highest sequence number runs ahead of
what the node log holds while groups wait for their window. In
crates/node/src/writer/, each `.max_seq_no()` call needs a `// ACKED:` or
`// QUIESCED:` comment on its line or within the three lines above: ACKED
when the value read is the acknowledged mark, QUIESCED when no group of the
shard is in flight. Code after `#[cfg(test)]` in a file is exempt.

Cannot catch: a comment that names the wrong reason, or a read through a
differently named accessor. A reviewer reads the comment against the call.
"""
import re
import sys

from checklib import read, report, tracked, window_has

CALL = re.compile(r"\.max_seq_no\s*\(")


def main():
    hits = []
    for rel in tracked({".rs"}):
        if not rel.startswith("crates/node/src/writer/"):
            continue
        lines = read(rel).splitlines()
        for i, line in enumerate(lines):
            if "#[cfg(test)]" in line:
                break
            code = line.split("//", 1)[0]
            if CALL.search(code) and not (
                window_has(lines, i, "// ACKED:") or window_has(lines, i, "// QUIESCED:")
            ):
                hits.append(f"{rel}:{i + 1}: max_seq_no() read without an '// ACKED:' or '// QUIESCED:' reason")
    return report("check-acked", hits)


if __name__ == "__main__":
    sys.exit(main())
