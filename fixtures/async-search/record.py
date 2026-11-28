#!/usr/bin/env python3
"""Records the asynchronous search plugin's answers from a running stock OpenSearch 3.10.

    python3 fixtures/async-search/record.py --url http://localhost:9200 fixtures/async-search/async-search.json

The plugin's ids are generated, so a step cannot name one in advance: a step may
`capture` the id of its answer under a label, and a later step writes it as
`{id:label}` in its path. The recording keeps the placeholder, never the id, and
replaces ids in answers with `<id:label>`. Times in answers are replaced by
`<time>`. Requests are written here; responses only ever come from the node.
Not run by the gate. Standard library only.
"""
import argparse, json, re, sys, time, urllib.error, urllib.parse, urllib.request

A = "/_plugins/_asynchronous_search"
IDX = "compat-async"
SLOWIDX = "compat-async-slow"
SLOW = {"query": {"function_score": {"query": {"match_all": {}}, "script_score": {"script": {
    "source": "long s=0; for(int i=0;i<900000;i++){s+=i;} return s%7;"}}}}}

MID = {"query": {"function_score": {"query": {"range": {"a": {"lt": 300}}}, "script_score": {"script": {
    "source": "long s=0; for(int i=0;i<900000;i++){s+=i;} return s%7;"}}}}}


def step(name, method, path, body=None, query=None, capture=None, wait=0):
    return dict(name=name, method=method, path=path, body=body, query=query or {}, capture=capture, wait=wait)


STEPS = [
    step("submit finishing inside the wait", "POST", f"/{IDX}{A}", {"query": {"match_all": {}}, "sort": [{"a": "asc"}]}, capture="plain"),
    step("get after a finished search that was not kept", "GET", f"{A}/{{id:plain}}"),
    step("delete after a finished search that was not kept", "DELETE", f"{A}/{{id:plain}}"),
    step("submit keep_on_completion", "POST", f"/{IDX}{A}", {"query": {"match_all": {}}, "sort": [{"a": "asc"}]},
         {"keep_on_completion": "true", "wait_for_completion_timeout": "10s"}, capture="kept"),
    step("get kept", "GET", f"{A}/{{id:kept}}", wait=1),
    step("get kept with keep_alive", "GET", f"{A}/{{id:kept}}", query={"keep_alive": "2d"}),
    step("get kept with wait_for_completion_timeout", "GET", f"{A}/{{id:kept}}", query={"wait_for_completion_timeout": "1s"}),
    step("delete kept", "DELETE", f"{A}/{{id:kept}}"),
    step("get deleted", "GET", f"{A}/{{id:kept}}"),
    step("delete deleted", "DELETE", f"{A}/{{id:kept}}"),
    step("submit with aggregation and size 0", "POST", f"/{IDX}{A}", {"size": 0, "aggs": {"m": {"max": {"field": "a"}}}},
         {"keep_on_completion": "true"}, capture="agg"),
    step("get aggregation", "GET", f"{A}/{{id:agg}}", wait=1),
    step("delete aggregation", "DELETE", f"{A}/{{id:agg}}"),
    step("submit still running", "POST", f"/{SLOWIDX}{A}", SLOW,
         {"wait_for_completion_timeout": "1ms", "keep_alive": "2m", "keep_on_completion": "true"}, capture="slow"),
    step("get running", "GET", f"{A}/{{id:slow}}"),
    step("stats with a running search", "GET", f"{A}/stats"),
    step("delete running", "DELETE", f"{A}/{{id:slow}}", wait=1),
    step("get cancelled", "GET", f"{A}/{{id:slow}}"),
    step("submit still running without keep_on_completion", "POST", f"/{SLOWIDX}{A}", SLOW,
         {"wait_for_completion_timeout": "1ms", "keep_alive": "2m"}, capture="slow2"),
    step("get running not kept", "GET", f"{A}/{{id:slow2}}"),
    step("delete running not kept", "DELETE", f"{A}/{{id:slow2}}", wait=1),
    step("submit that finishes after the wait", "POST", f"/{SLOWIDX}{A}", MID,
         {"wait_for_completion_timeout": "1ms", "keep_alive": "2m", "keep_on_completion": "true"}, capture="mid"),
    step("get a search that finished after the wait", "GET", f"{A}/{{id:mid}}", wait=6),
    step("delete it", "DELETE", f"{A}/{{id:mid}}"),
    step("submit that finishes after the wait and is not kept", "POST", f"/{SLOWIDX}{A}", MID,
         {"wait_for_completion_timeout": "1ms", "keep_alive": "2m"}, capture="mid2"),
    step("get a not kept search that finished after the wait", "GET", f"{A}/{{id:mid2}}", wait=6),
    step("get it again", "GET", f"{A}/{{id:mid2}}"),
    step("submit failing query", "POST", f"/{IDX}{A}", {"query": {"nope": {}}}, {"keep_on_completion": "true", "wait_for_completion_timeout": "10s"}, capture="bad"),
    step("get failing", "GET", f"{A}/{{id:bad}}", wait=1),
    step("delete failing", "DELETE", f"{A}/{{id:bad}}"),
    step("submit failing query not kept", "POST", f"/{IDX}{A}", {"query": {"nope": {}}}),
    step("submit with a malformed body", "POST", f"/{IDX}{A}", None),
    step("submit to a missing index", "POST", f"/compat-async-missing{A}", {"query": {"match_all": {}}}),
    step("submit to a wildcard", "POST", f"/compat-async*{A}", {"query": {"match_all": {}}}),
    step("submit to every index", "POST", A, {"size": 0}),
    step("submit with keep_alive over the limit", "POST", f"/{IDX}{A}", {"query": {"match_all": {}}}, {"keep_alive": "100d"}),
    step("submit with keep_alive below the minimum", "POST", f"/{IDX}{A}", {"query": {"match_all": {}}}, {"keep_alive": "1ms"}),
    step("submit with a bad keep_alive", "POST", f"/{IDX}{A}", {"query": {"match_all": {}}}, {"keep_alive": "soon"}),
    step("submit with a bad wait_for_completion_timeout", "POST", f"/{IDX}{A}", {"query": {"match_all": {}}}, {"wait_for_completion_timeout": "soon"}),
    step("submit with a bad keep_on_completion", "POST", f"/{IDX}{A}", {"query": {"match_all": {}}}, {"keep_on_completion": "maybe"}),
    step("submit with an unknown parameter", "POST", f"/{IDX}{A}", {"query": {"match_all": {}}}, {"nope": "1"}),
    step("submit with batched_reduce_size", "POST", f"/{IDX}{A}", {"query": {"match_all": {}}}, {"batched_reduce_size": "2", "keep_on_completion": "true"}, capture="b"),
    step("delete batched", "DELETE", f"{A}/{{id:b}}", wait=1),
    step("submit with request_cache true", "POST", f"/{IDX}{A}", {"query": {"match_all": {}}}, {"request_cache": "true"}),
    step("submit with routing and preference", "POST", f"/{IDX}{A}", {"query": {"match_all": {}}}, {"routing": "r", "preference": "_local"}),
    step("get a garbage id", "GET", f"{A}/not-an-id"),
    step("delete a garbage id", "DELETE", f"{A}/not-an-id"),
    step("get with no id", "GET", A),
    step("stats", "GET", f"{A}/stats"),
    step("stats of one node", "GET", f"{A}/_local/stats"),
    step("legacy route", "POST", f"/{IDX}/_opendistro/_asynchronous_search", {"query": {"match_all": {}}}),
]


def send(base, s, ids):
    path = re.sub(r"\{id:(\w+)\}", lambda m: ids.get(m.group(1), "no-id-captured"), s["path"])
    url = base + urllib.parse.quote(path, safe="/_-.,*") + ("?" + urllib.parse.urlencode(sorted(s["query"].items())) if s["query"] else "")
    data = json.dumps(s["body"]).encode() if s["body"] is not None else None
    req = urllib.request.Request(url, data=data, method=s["method"], headers={"content-type": "application/json"} if data is not None else {})
    try:
        r = urllib.request.urlopen(req, timeout=120)
    except urllib.error.HTTPError as e:
        r = e
    raw = r.read()
    try:
        body = json.loads(raw) if raw.strip() else None
    except ValueError:
        body = raw.decode()
    return r.status, body


def scrub(v, ids):
    inv = {i: f"<id:{k}>" for k, i in ids.items()}
    if isinstance(v, str):
        for i, ph in inv.items():
            v = v.replace(i, ph)
        return v
    if isinstance(v, list):
        return [scrub(x, ids) for x in v]
    if isinstance(v, dict):
        out = {}
        for k, x in v.items():
            if k in ("start_time_in_millis", "expiration_time_in_millis", "took", "took_in_millis"):
                out[k] = "<volatile>"
            elif k in ("submitted","initialized","persisted","search_failed","search_completed","rejected","persist_failed","cancelled"):
                out[k] = "<counter>"
            elif k == "id" and isinstance(x, str) and x not in inv:
                out[k] = "<id:unnamed>"
            elif k == "nodes" and isinstance(x, dict) and all(isinstance(n, dict) for n in x.values()):
                out[k] = {"<node>": scrub(n, ids) for n in x.values()}
            else:
                out[k] = scrub(x, ids)
        return out
    return v


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--url", default="http://localhost:9200")
    ap.add_argument("out")
    a = ap.parse_args()
    _, root = send(a.url, dict(method="GET", path="/", body=None, query={}), {})
    v = root["version"]
    for m in ("DELETE",):
        send(a.url, dict(method=m, path=f"/{IDX}", body=None, query={"ignore_unavailable": "true"}), {})
    send(a.url, dict(method="PUT", path=f"/{IDX}", body={"settings": {"number_of_shards": 1, "number_of_replicas": 0}}, query={}), {})
    nd = "\n".join(json.dumps(x) for x in [{"index": {"_id": "1"}}, {"a": 1}, {"index": {"_id": "2"}}, {"a": 2}, {"index": {"_id": "3"}}, {"a": 3}]) + "\n"
    urllib.request.urlopen(urllib.request.Request(a.url + f"/{IDX}/_bulk?refresh=true", data=nd.encode(), method="POST", headers={"content-type": "application/x-ndjson"}))
    send(a.url, dict(method="DELETE", path=f"/{SLOWIDX}", body=None, query={"ignore_unavailable": "true"}), {})
    send(a.url, dict(method="PUT", path=f"/{SLOWIDX}", body={"settings": {"number_of_shards": 1, "number_of_replicas": 0}}, query={}), {})
    nd = "\n".join(json.dumps(x) for i in range(4000) for x in ({"index": {"_id": str(i)}}, {"a": i})) + "\n"
    urllib.request.urlopen(urllib.request.Request(a.url + f"/{SLOWIDX}/_bulk?refresh=true", data=nd.encode(), method="POST", headers={"content-type": "application/x-ndjson"}), timeout=120)
    ids, steps = {}, []
    for s in STEPS:
        if s["wait"]:
            time.sleep(s["wait"])
        status, body = send(a.url, s, ids)
        if s["capture"] and isinstance(body, dict) and "id" in body:
            ids[s["capture"]] = body["id"]
        steps.append({"name": s["name"],
                      "request": {k: s[k] for k in ("method", "path", "query", "body") if s[k] not in (None, {})},
                      "response": {"status": status, "body": scrub(body, ids)}})
    send(a.url, dict(method="DELETE", path=f"/{IDX}", body=None, query={}), {})
    send(a.url, dict(method="DELETE", path=f"/{SLOWIDX}", body=None, query={}), {})
    out = {"name": "asynchronous search routes", "description": "Submit, get, delete and stats of the stock plugin: states, what is kept, validation, and the answers for ids that do not exist. Ids are placeholders.",
           "recorded_from": {"distribution": v.get("distribution"), "version": v["number"], "build_hash": v["build_hash"]},
           "setup": f"index {IDX}: one shard, three documents with a = 1, 2, 3; index {SLOWIDX}: one shard, 4,000 documents, searched with a script that loops 900,000 times a document so a search runs for seconds", "steps": steps}
    json.dump(out, open(a.out, "w"), indent=2)
    open(a.out, "a").write("\n")


main()
