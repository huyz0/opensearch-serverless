#!/usr/bin/env python3
"""S3 is reached only through oss-blob, and every LIST is justified.

1. Only crates/blob may name an S3 client crate as a dependency, so every
   request passes the meter there.
2. A LIST call site in Rust (`list_objects`, `.list(`, `list_prefix`) has a
   `// LIST:` comment on its line or within the three lines above, saying why
   a derived key or an existing read cannot answer instead. LIST is priced
   like a PUT.

Cannot catch: a LIST under a name these patterns miss, or a client crate
pulled in transitively under another name.
"""
import re
import sys

from checklib import ROOT, read, report, tracked, window_has

CLIENTS = {"aws-sdk-s3", "object_store", "rusoto_s3", "rust-s3", "s3", "minio", "opendal"}
ALLOWED = {"crates/blob/Cargo.toml"}
LIST_CALL = re.compile(r"\blist_objects\w*\s*\(|\.list\s*\(|\blist_prefix\w*\s*\(")
DEP_LINE = re.compile(r"^\s*([A-Za-z0-9_-]+)\s*=")


def main():
    hits = []
    for manifest in sorted(ROOT.glob("**/Cargo.toml")):
        rel = manifest.relative_to(ROOT).as_posix()
        if rel.startswith("target") or rel in ALLOWED:
            continue
        for n, line in enumerate(manifest.read_text(encoding="utf-8").splitlines(), 1):
            m = DEP_LINE.match(line)
            if m and m.group(1) in CLIENTS:
                hits.append(f"{rel}:{n}: S3 client '{m.group(1)}' outside oss-blob; go through oss-blob so the call is metered")
    for rel in tracked({".rs"}):
        lines = read(rel).splitlines()
        for i, line in enumerate(lines):
            if LIST_CALL.search(line) and not line.lstrip().startswith("//"):
                if not window_has(lines, i, "// LIST:"):
                    hits.append(f"{rel}:{i + 1}: LIST call without a '// LIST:' justification")
    return report("check-s3-access", hits)


if __name__ == "__main__":
    sys.exit(main())
