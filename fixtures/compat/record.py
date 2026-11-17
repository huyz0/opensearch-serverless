#!/usr/bin/env python3
"""Records fixture responses from a running stock OpenSearch.

    python3 fixtures/compat/record.py --url http://localhost:9200 fixtures/compat/*/*.json

For each fixture file: reads the server's version from `GET /`, sends the
fixture's cleanup requests, sends every step and writes its status, selected
headers and JSON body into the step's `response`, sends the cleanup again,
and writes `recorded_from`. Requests and normalisation rules are written by
hand; responses only ever come from here. Each rule must match something in
the new recording, or the file is left unchanged and the run fails.

Not run by the gate: it needs a live server. The procedure, including which
OpenSearch build to record from, is in fixtures/compat/README.md.

Standard library only.
"""
import argparse
import json
import sys
import urllib.error
import urllib.parse
import urllib.request

DEFAULT_HEADERS = ["content-type"]
MAX_RESPONSE_BYTES = 1 << 20


def send(base, request):
    """Sends one fixture request; returns (status, headers, body or None)."""
    url = base.rstrip("/") + urllib.parse.quote(request["path"], safe="/_-.,*")
    if request.get("query"):
        url += "?" + urllib.parse.urlencode(sorted(request["query"].items()))
    data, ctype = None, None
    if "body" in request:
        data, ctype = json.dumps(request["body"], separators=(",", ":")).encode(), "application/json"
    elif "raw" in request:
        # A body that is no single JSON value (duplicate keys, trailing data), sent as written.
        data, ctype = request["raw"].encode(), "application/json"
    elif "ndjson" in request:
        lines = (json.dumps(line, separators=(",", ":")) for line in request["ndjson"])
        data, ctype = ("\n".join(lines) + "\n").encode(), "application/x-ndjson"
    req = urllib.request.Request(url, data=data, method=request["method"])
    if ctype:
        req.add_header("Content-Type", ctype)
    try:
        resp = urllib.request.urlopen(req, timeout=60)
    except urllib.error.HTTPError as e:
        resp = e
    with resp:
        raw = resp.read(MAX_RESPONSE_BYTES + 1)
        if len(raw) > MAX_RESPONSE_BYTES:
            raise SystemExit(f"record: {request['path']}: response over {MAX_RESPONSE_BYTES} bytes")
        headers = {k.lower(): v for k, v in resp.headers.items()}
        body = json.loads(raw) if raw.strip() else None
        return resp.status, headers, body


def pointer_matches(doc, segments):
    """How many values a fixture pointer (with `*` segments) matches."""
    if not segments:
        return 1
    head, rest = segments[0], segments[1:]
    if head == "*":
        children = doc if isinstance(doc, list) else list(doc.values()) if isinstance(doc, dict) else []
        return sum(pointer_matches(c, rest) for c in children)
    key = head.replace("~1", "/").replace("~0", "~")
    if isinstance(doc, dict) and key in doc:
        return pointer_matches(doc[key], rest)
    if isinstance(doc, list) and key.isdigit() and int(key) < len(doc):
        return pointer_matches(doc[int(key)], rest)
    return 0


def record(base, path, server):
    with open(path, encoding="utf-8") as f:
        fixture = json.load(f)
    for request in fixture.get("cleanup", []):
        send(base, request)
    problems = []
    for step in fixture["steps"]:
        selected = list((step.get("response") or {}).get("headers") or DEFAULT_HEADERS)
        status, headers, body = send(base, step["request"])
        response = {"status": status, "headers": {h: headers[h] for h in selected if h in headers}}
        if body is not None:
            response["body"] = body
        step["response"] = response
        doc = {"status": status, "headers": response["headers"], "body": body}
        for rule in step.get("normalise", []):
            if pointer_matches(doc, rule["pointer"].split("/")[1:]) == 0:
                problems.append(f"{path}: step {step['name']!r}: rule {rule['pointer']} matches nothing")
    for request in fixture.get("cleanup", []):
        send(base, request)
    if problems:
        return problems
    fixture["recorded_from"] = server
    keys = ["name", "description", "recorded_from", "cleanup", "steps"]
    ordered = {k: fixture[k] for k in keys if k in fixture}
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        json.dump(ordered, f, indent=2, ensure_ascii=False)
        f.write("\n")
    return []


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--url", default="http://localhost:9200")
    parser.add_argument("--expect-version", default="3.10.0",
                        help="refuse a server whose version does not start with this")
    parser.add_argument("fixtures", nargs="+")
    args = parser.parse_args()
    _, _, root = send(args.url, {"method": "GET", "path": "/"})
    version = root["version"]
    if not version["number"].startswith(args.expect_version):
        raise SystemExit(f"record: server is {version['number']}, expected {args.expect_version}")
    server = {"distribution": version.get("distribution", "opensearch"),
              "version": version["number"], "build_hash": version["build_hash"]}
    problems = []
    for path in args.fixtures:
        found = record(args.url, path, server)
        problems += found
        print(f"record: {path}: {'FAILED' if found else 'recorded'}")
    for p in problems:
        print(p, file=sys.stderr)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
