#!/usr/bin/env python3
"""Records the Lucene segment a stock OpenSearch writes for a fixture's documents.

    python3 fixtures/segments/record.py --url http://localhost:9200 \
        --data /path/to/opensearch/data --lucene /path/to/lucene-core.jar \
        fixtures/segments/*.json

For each fixture file (`fixtures/segments/<name>.json`): deletes and creates its index
with one shard, no replicas and no refresh, and the fixture's `mappings`;
indexes its `documents` in order in one `_bulk` (one request, so one indexing
thread and one segment); flushes; copies the shard's Lucene files to a
temporary directory; and writes what `SegmentDump.java` reads from them into
the fixture's `segment`, with `recorded_from`. The server must run on this
machine, with `--data` its `path.data`, so the shard's directory can be read,
and `--lucene` the lucene-core jar of the same build (its `lib/`).

Not run by the gate: it needs a live server. The procedure is in
fixtures/segments/README.md.

Standard library only.
"""
import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile

import importlib.util

HERE = os.path.dirname(os.path.abspath(__file__))
# fixtures/compat/record.py's `send`, loaded by path: this file has its name.
_spec = importlib.util.spec_from_file_location("compat_record", os.path.join(HERE, "..", "compat", "record.py"))
_compat = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_compat)
send = _compat.send

KEYS = ["name", "description", "recorded_from", "index", "mappings", "documents", "segment"]
DUMP = os.path.join(HERE, "SegmentDump.java")


def check(status, body, what):
    if status >= 300:
        raise SystemExit(f"record: {what}: {status} {json.dumps(body)[:2000]}")


def record(base, data, lucene, path, server):
    with open(path, encoding="utf-8") as f:
        fixture = json.load(f)
    unknown = set(fixture) - set(KEYS)
    if unknown:
        raise SystemExit(f"record: {path}: unknown keys {sorted(unknown)}")
    index = fixture["index"]
    send(base, {"method": "DELETE", "path": f"/{index}", "query": {"ignore_unavailable": "true"}})
    create = {"settings": {"number_of_shards": 1, "number_of_replicas": 0, "refresh_interval": "-1"}}
    if "mappings" in fixture:
        create["mappings"] = fixture["mappings"]
    check(*send(base, {"method": "PUT", "path": f"/{index}", "body": create})[::2], "create")
    lines = []
    for doc in fixture["documents"]:
        if doc.get("delete"):
            lines.append({"delete": {"_index": index, "_id": doc["id"]}})
        else:
            lines += [{"index": {"_index": index, "_id": doc["id"]}}, doc["source"]]
    status, _, body = send(base, {"method": "POST", "path": "/_bulk", "ndjson": lines})
    check(status, body, "bulk")
    items = [next(iter(i.values())) for i in body["items"]]
    failed = [i for i in items if i["status"] >= 300 and i.get("result") != "not_found"]
    if failed:
        raise SystemExit(f"record: {path}: refused documents {json.dumps(failed)[:2000]}")
    check(*send(base, {"method": "POST", "path": f"/{index}/_flush", "query": {"force": "true"}})[::2], "flush")
    status, _, settings = send(base, {"method": "GET", "path": f"/{index}/_settings"})
    check(status, settings, "settings")
    uuid = settings[index]["settings"]["index"]["uuid"]
    shard = os.path.join(data, "nodes", "0", "indices", uuid, "0", "index")
    with tempfile.TemporaryDirectory() as out:
        for name in sorted(os.listdir(shard)):
            if name != "write.lock":
                shutil.copyfile(os.path.join(shard, name), os.path.join(out, name))
        dump = subprocess.run(["java", "-cp", lucene, DUMP, out], check=True,
                              capture_output=True, text=True).stdout
    fixture["segment"] = json.loads(dump)
    send(base, {"method": "DELETE", "path": f"/{index}"})
    fixture["recorded_from"] = server
    ordered = {k: fixture[k] for k in KEYS if k in fixture}
    with open(path, "w", encoding="utf-8") as f:
        json.dump(ordered, f, indent=2, ensure_ascii=False)
        f.write("\n")
    return f"{fixture['segment']['max_doc']} documents, {len(fixture['segment']['fields'])} fields"


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--url", default="http://localhost:9200")
    parser.add_argument("--data", required=True, help="the server's path.data")
    parser.add_argument("--lucene", required=True, help="the server's lucene-core jar")
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
    for path in args.fixtures:
        summary = record(args.url, args.data, args.lucene, path, server)
        print(f"record: {path}: {summary}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
