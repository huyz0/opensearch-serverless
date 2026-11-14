#!/usr/bin/env python3
"""Every allow of `clippy::arithmetic_side_effects` carries a proof.

The allow needs a `// ARITH:` comment on its line or within the three lines
above, saying why the operation cannot overflow (for example, both operands
were bounded by a check just above).

Cannot catch: a proof that is wrong.
"""
import sys

from checklib import read, report, tracked, window_has

NEEDLE = "clippy::arithmetic_side_effects"


def main():
    hits = []
    for rel in tracked({".rs"}):
        lines = read(rel).splitlines()
        for i, line in enumerate(lines):
            if NEEDLE in line and "allow" in line and not window_has(lines, i, "// ARITH:"):
                hits.append(f"{rel}:{i + 1}: arithmetic allow without an '// ARITH:' proof")
    return report("check-arith-allows", hits)


if __name__ == "__main__":
    sys.exit(main())
