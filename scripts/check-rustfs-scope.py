#!/usr/bin/env python3
"""RustFS is for S3-compatibility checks, not for perf, scale or lifecycle tests.

A Rust file that reads `RUSTFS_BIN` must be listed in scripts/rustfs-scope.txt
with a category (compat, cross, twin), and a listed file must still read it.
Adding a RustFS-backed test therefore shows in the diff of that list, where a
reviewer asks why the emulator or the fs store, behind the same BlobStore trait
and with the same request metering, is not enough.

Cannot catch: a test that starts RustFS without the variable, or a listed file
that gains an unrelated RustFS test.
"""
import sys

from checklib import read, report, tracked

LIST = "scripts/rustfs-scope.txt"
CATEGORIES = {"compat", "cross", "twin"}


def listed():
    entries, problems = {}, []
    for n, line in enumerate(read(LIST).splitlines(), 1):
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        parts = line.split()
        if len(parts) != 2 or parts[1] not in CATEGORIES:
            problems.append(f"{LIST}:{n}: expected `<path> compat|cross|twin`")
            continue
        entries[parts[0]] = parts[1]
    return entries, problems


def main():
    entries, hits = listed()
    users = {rel for rel in tracked({".rs"}) if "RUSTFS_BIN" in read(rel)}
    for rel in sorted(users - set(entries)):
        hits.append(f"{rel}: reads RUSTFS_BIN but is not in {LIST}; RustFS is for S3-compat checks, run perf and scale on the emulator or fs store")
    for rel in sorted(set(entries) - users):
        hits.append(f"{LIST}: {rel} no longer reads RUSTFS_BIN; remove the line")
    return report("check-rustfs-scope", hits)


if __name__ == "__main__":
    sys.exit(main())
