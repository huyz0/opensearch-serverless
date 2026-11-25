#!/usr/bin/env python3
"""No shipped configuration turns analyzer pins on.

`serverless.extensions.pins.allowed` stays false until the soak, the replay
equality run and the measurements of ADR 0053 are recorded, because an index
that pins a module cannot be served by a Java node. Fails on a line of a
configuration or script that names the setting beside `true`. Rust sources and
Markdown are not scanned: tests set it, and the docs describe it.

Cannot catch: a value assembled at run time, or set through the REST API.
"""
import re
import sys

from checklib import read, report, tracked

SCANNED = {".toml", ".yml", ".yaml", ".sh", ".json", ".env", ".properties", ".conf"}
ON = re.compile(r"serverless\.extensions\.pins\.allowed[\"']?\s*[:=]\s*[\"']?true\b")
EXEMPT = {"scripts/check-extension-pins.py", "scripts/selftest-checks.py"}  # they hold planted examples


def main():
    hits = []
    for rel in tracked(SCANNED):
        if rel in EXEMPT or rel.startswith("docs/"):
            continue
        for n, line in enumerate(read(rel).splitlines(), 1):
            if ON.search(line):
                hits.append(f"{rel}:{n}: turns serverless.extensions.pins.allowed on in a shipped configuration")
    return report("check-extension-pins", hits)


if __name__ == "__main__":
    sys.exit(main())
