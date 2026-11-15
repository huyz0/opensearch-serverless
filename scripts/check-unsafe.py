#!/usr/bin/env python3
"""`unsafe` is used only in the narrow, documented way docs/unsafe.md allows.

Any crate may use `unsafe`, item by item. Fails on:
- a module- or crate-wide `#![allow|expect|warn(unsafe_code)]`, or an
  item-level `warn` (the allow must sit on the smallest item);
- an `unsafe_code` lint level in a manifest other than "deny" or "forbid";
- `unsafe impl Send` / `unsafe impl Sync`, `transmute`, and `extern` blocks,
  which need the owner's written OK: an approved use is listed in APPROVED
  as "<path>:<what>" in the PR that carries the OK.

The SAFETY comment on every block and the one-operation-per-block rule are
clippy's (`undocumented_unsafe_blocks`, `multiple_unsafe_ops_per_block`).

Cannot catch: an unsafe block that is not one of the five allowed patterns,
a SAFETY comment that is false, unsafe code in a dependency, or a
`transmute` reached through a renamed import. Review and Miri cover those.
"""
import re
import sys

from checklib import read, report, tracked

# Owner-approved exceptions, "<path>:<what>", for example
# "crates/ffi/src/lib.rs:extern". Empty today.
APPROVED = set()

WIDE_ALLOW = re.compile(r"#!\[\s*(?:allow|expect|warn)\s*\([^)]*\bunsafe_code\b")
ITEM_WARN = re.compile(r"#\[\s*warn\s*\([^)]*\bunsafe_code\b")
RELAX_TOML = re.compile(r"""^\s*unsafe_code\s*=(?!\s*"(?:forbid|deny)")""")
NEEDS_OK = [
    ("send-sync", re.compile(r"\bunsafe\s+impl\b[^{]*\b(?:Send|Sync)\b"), "unsafe impl Send/Sync"),
    ("transmute", re.compile(r"\btransmute(?:_copy)?\b"), "transmute (use bytemuck or zerocopy)"),
    ("extern", re.compile(r"""\b(?:unsafe\s+)?extern\s+"[^"]*"\s*\{"""), "an extern block (FFI)"),
]


def main():
    hits = []
    for rel in tracked({".rs", ".toml"}):
        for n, line in enumerate(read(rel).splitlines(), 1):
            if rel.endswith(".toml"):
                if RELAX_TOML.search(line):
                    hits.append(f"{rel}:{n}: unsafe_code must stay \"deny\" (or \"forbid\")")
                continue
            code = line.split("//", 1)[0]
            if WIDE_ALLOW.search(code):
                hits.append(f"{rel}:{n}: module- or crate-wide unsafe_code allow; put #[allow(unsafe_code)] on the smallest item")
            elif ITEM_WARN.search(code):
                hits.append(f"{rel}:{n}: warn(unsafe_code) relaxes the lint; use #[allow(unsafe_code)] on the item")
            for key, pattern, what in NEEDS_OK:
                if pattern.search(code) and f"{rel}:{key}" not in APPROVED:
                    hits.append(f"{rel}:{n}: {what} needs the owner's OK (docs/unsafe.md)")
    return report("check-unsafe", hits)


if __name__ == "__main__":
    sys.exit(main())
