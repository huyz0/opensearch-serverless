#!/usr/bin/env python3
"""Tests and scripts run against emulators (RustFS, MinIO), never real AWS.

Fails on an AWS endpoint host in code, configuration or scripts. docs/ and
Markdown may mention AWS and are not scanned.

Cannot catch: an endpoint assembled at run time, or one from the environment.
"""
import re
import sys

from checklib import read, report, tracked

SCANNED = {".rs", ".toml", ".yml", ".yaml", ".sh", ".py", ".json"}
AWS = re.compile(r"amazonaws\.com|aws\.amazon\.com")
EXEMPT = {
    "scripts/check-test-endpoints.py",
    "scripts/selftest-checks.py",  # they hold planted examples
    # The notifications SNS and SES client names the public service host a deployment sends to;
    # its tests set the account's `endpoint` to a local server and never reach it.
    "crates/node/src/notifications_route/aws.rs",
}


def main():
    hits = []
    for rel in tracked(SCANNED):
        if rel in EXEMPT or rel.startswith("docs/"):
            continue
        for n, line in enumerate(read(rel).splitlines(), 1):
            if AWS.search(line):
                hits.append(f"{rel}:{n}: real AWS endpoint; tests run on RustFS or another emulator")
    return report("check-test-endpoints", hits)


if __name__ == "__main__":
    sys.exit(main())
