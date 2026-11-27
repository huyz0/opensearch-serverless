#!/usr/bin/env python3
"""A merged integration-test binary holds no global allocator.

A crate's tests live in one binary, `tests/it/` ([ADR 0070](../docs/adr/0070-each-crate-builds-its-integration-tests-as-one-binary.md)).
A `#[global_allocator]` is a property of the whole binary, so a file that
installs one would change every other test's allocator and the memory it
measures: such a file stays a test target of its own, outside `tests/it/`.

Cannot catch: a test that reads process-global state (an environment variable,
a static) and breaks beside the others; the suite run under both runners does.
"""
import re
import sys

from checklib import read, report, tracked

ALLOCATOR = re.compile(r"#\[\s*global_allocator\s*\]")


def main():
    hits = []
    for rel in tracked({".rs"}):
        if "/tests/it/" in rel and ALLOCATOR.search(read(rel)):
            hits.append(f"{rel}: a global allocator in the merged test binary; make it a test target of its own outside tests/it/")
    return report("check-test-binaries", hits)


if __name__ == "__main__":
    sys.exit(main())
