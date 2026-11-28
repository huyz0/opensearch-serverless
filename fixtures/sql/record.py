#!/usr/bin/env python3
"""Records the SQL and PPL plugin's answers for the corpus in `corpus.txt`.

    python3 fixtures/sql/record.py --url http://localhost:9200 fixtures/sql

Needs a stock OpenSearch 3.10.0 with the SQL plugin (the release tarball).
Creates the indices of `data.json`, then for each statement of `corpus.txt`
writes beside it `NAME.sql`, `NAME.dsl.json` (the request body the plugin
sends to the search, taken from `_explain`), `NAME.jdbc.json`, `NAME.csv` and
`NAME.raw` (the same statement with format=csv and format=raw), or
`NAME.error.json` when the plugin refuses it. Every file starts with a header
or carries `"recorded_from"`. Responses only ever come from the node.
Standard library only.
"""
import argparse, json, os, re, sys, urllib.error, urllib.request


def call(base, method, path, body=None, raw=None, ctype="application/json"):
    data = raw if raw is not None else (json.dumps(body).encode() if body is not None else None)
    req = urllib.request.Request(base + path, data=data, method=method, headers={"content-type": ctype} if data is not None else {})
    try:
        r = urllib.request.urlopen(req, timeout=120)
    except urllib.error.HTTPError as e:
        r = e
    return r.status, r.read().decode()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--url", default="http://localhost:9200")
    ap.add_argument("dir")
    a = ap.parse_args()
    _, root = call(a.url, "GET", "/")
    v = json.loads(root)["version"]
    server = {"distribution": v["distribution"], "version": v["number"], "build_hash": v["build_hash"]}
    data = json.load(open(os.path.join(a.dir, "data.json")))
    for name, idx in data["indices"].items():
        call(a.url, "DELETE", f"/{name}?ignore_unavailable=true")
        s, b = call(a.url, "PUT", f"/{name}", {"settings": {"number_of_shards": 1, "number_of_replicas": 0}, "mappings": idx["mappings"]})
        assert s == 200, b
        nd = "".join(json.dumps(x) + "\n" for i, d in enumerate(idx["docs"]) for x in ({"index": {"_id": str(i + 1)}}, d))
        s, b = call(a.url, "POST", f"/{name}/_bulk?refresh=true", raw=nd.encode(), ctype="application/x-ndjson")
        assert s == 200 and not json.loads(b)["errors"], b
    out = {}
    for line in open(os.path.join(a.dir, "corpus.txt")):
        line = line.rstrip("\n")
        if not line or line.startswith("#"):
            continue
        name, lang, stmt = line.split("|", 2)
        base = "/_plugins/_sql" if lang == "sql" else "/_plugins/_ppl"
        q = {"query": stmt}
        st, jb = call(a.url, "POST", base, q)
        stem = os.path.join(a.dir, "cases", name)
        os.makedirs(os.path.dirname(stem), exist_ok=True)
        open(stem + "." + lang, "w").write(stmt + "\n")
        for ext in ("jdbc.json", "dsl.json", "error.json", "csv", "raw"):
            if os.path.exists(f"{stem}.{ext}"):
                os.remove(f"{stem}.{ext}")
        if st != 200:
            body = json.loads(jb)
            json.dump({"recorded_from": server, "status": st, "body": body}, open(stem + ".error.json", "w"), indent=2)
            open(stem + ".error.json", "a").write("\n")
            continue
        body = json.loads(jb)
        json.dump({"recorded_from": server, "response": body}, open(stem + ".jdbc.json", "w"), indent=2)
        open(stem + ".jdbc.json", "a").write("\n")
        st2, eb = call(a.url, "POST", base + "/_explain", q)
        if st2 == 200:
            try:
                plan = json.loads(eb)
            except ValueError:
                plan = eb
            dsl = {"recorded_from": server, "explain": plan}
            m = re.search(r"sourceBuilder=(\{.*\}), (?:pitId|searchAfter)=", plan_text(plan))
            if m:
                try:
                    dsl["dsl"] = json.loads(m.group(1))
                except ValueError:
                    pass
            json.dump(dsl, open(stem + ".dsl.json", "w"), indent=2)
            open(stem + ".dsl.json", "a").write("\n")
        for fmt, ext in (("csv", "csv"), ("raw", "raw")):
            st3, tb = call(a.url, "POST", base + "?format=" + fmt, q)
            open(f"{stem}.{ext}", "w").write(tb if st3 == 200 else f"# status {st3}\n{tb}")
    for name in data["indices"]:
        call(a.url, "DELETE", f"/{name}")


def plan_text(plan):
    """The request string of the scan node in an explain plan."""
    def walk(n):
        if isinstance(n, dict):
            d = n.get("description")
            if isinstance(d, dict) and "request" in d:
                return d["request"]
            for c in n.get("children", []) or []:
                r = walk(c)
                if r:
                    return r
            return walk(n["root"]) if "root" in n else None
    return (walk(plan) if isinstance(plan, dict) else None) or json.dumps(plan)


main()
