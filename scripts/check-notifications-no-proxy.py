#!/usr/bin/env python3
"""The notifications port never reaches a Java node (ADR 0081).

Java is a test oracle and a perf comparison, never in the runtime path: no
proxy and no fallback. A source file of `notifications_route` or
`notifications_setting` that names the plugin proxy or its upstream setting
fails.

Cannot catch: an upstream reached through a helper that hides its name.
"""
import re
import sys

from checklib import read, report, tracked

FORBIDDEN = re.compile(r"plugin_proxy|OSS_PLUGIN_UPSTREAMS|PLUGIN_PREFIXES")
SCOPE = ("crates/node/src/notifications_route/", "crates/node/src/notifications_setting.rs")


def main():
    hits = []
    for rel in tracked({".rs"}):
        if not rel.startswith(SCOPE):
            continue
        for n, line in enumerate(read(rel).splitlines(), 1):
            if FORBIDDEN.search(line):
                hits.append(f"{rel}:{n}: the notifications port must not use the Java plugin proxy (ADR 0081)")
    return report("check-notifications-no-proxy", hits)


if __name__ == "__main__":
    sys.exit(main())
