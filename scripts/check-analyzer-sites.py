#!/usr/bin/env python3
"""A built-in analyzer is built only where a pinned field is already handled.

An index can pin a module as a field's analyzer (ADR 0053), and a node must
never analyse that field's text with a built-in one. Each shipped site that
builds a built-in analyzer in `crates/node/src` or `crates/reader/src` is
listed here after being checked to route a pinned field to its module (or
refuse it): a query site hands pinned fields to `PinnedTokens`, highlighting
refuses them, and `_analyze` offers only the two built-in names. A new site
fails until it is reviewed and added. Test files are not scanned.

Cannot catch: an analyzer built through a helper that hides the constructor.
"""
import re
import sys

from checklib import read, report, tracked

CONSTRUCTOR = re.compile(r"\bAnalyzer::(standard|keyword|simple|whitespace|stop)\b")
SITES = {
    "crates/node/src/search.rs",
    "crates/node/src/search_analyze.rs",
    "crates/node/src/search_count.rs",
    "crates/node/src/search_frozen.rs",
    "crates/node/src/search_msearch.rs",
    "crates/node/src/search_shard.rs",
    "crates/node/src/search_validate.rs",
    "crates/reader/src/dsl/queries/query_string_build.rs",
    "crates/reader/src/fetch/highlight.rs",
    "crates/reader/src/pinned.rs",
}
ROOTS = ("crates/node/src/", "crates/reader/src/")


def is_test(rel):
    return rel.endswith(("_tests.rs", "/tests.rs")) or "/tests/" in rel


def main():
    hits = []
    for rel in tracked({".rs"}):
        if not rel.startswith(ROOTS) or is_test(rel) or rel in SITES:
            continue
        for n, line in enumerate(read(rel).splitlines(), 1):
            if CONSTRUCTOR.search(line):
                hits.append(f"{rel}:{n}: builds a built-in analyzer where no pinned field is handled; "
                            "route a pinned field to its module or refuse it, then list the file in SITES")
    return report("check-analyzer-sites", hits)


if __name__ == "__main__":
    sys.exit(main())
