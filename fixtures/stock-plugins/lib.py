"""Shared recorder for plugins whose answers carry generated ids.

A scenario is a list of `step(...)`. A step may `capture` values of its answer
under labels (`{"mon": "/_id"}`, a JSON pointer) and later steps write them as
`{mon}` in a path, a query value or any string of a body. The recording keeps
placeholders, never the generated values, and replaces every captured value found
in an answer with `<label>`. Keys named in `volatile` are replaced by
`<volatile>` wherever they appear. A step may `poll` an answer until a pointer
holds a value (the plugin does the work later), and only the final answer is kept.

Requests are written in the scenario; responses only ever come from the node.
Not run by the gate. Standard library only.
"""
import json, re, sys, time, urllib.error, urllib.parse, urllib.request


def step(name, method, path, body=None, query=None, capture=None, poll=None, wait=0, raw=None, ctype=None):
    return dict(name=name, method=method, path=path, body=body, query=query or {}, capture=capture or {}, poll=poll, wait=wait, raw=raw, ctype=ctype)


def sub(v, ids):
    if isinstance(v, str):
        return re.sub(r"\{(\w+)\}", lambda m: str(ids[m.group(1)]) if m.group(1) in ids else m.group(0), v)
    if isinstance(v, list):
        return [sub(x, ids) for x in v]
    if isinstance(v, dict):
        return {sub(k, ids): sub(x, ids) for k, x in v.items()}
    return v


def send(base, method, path, body=None, query=None, raw=None, ctype=None, auth=None):
    url = base + urllib.parse.quote(path, safe="/_-.,*:") + ("?" + urllib.parse.urlencode(sorted((query or {}).items())) if query else "")
    data = raw.encode() if raw is not None else (json.dumps(body).encode() if body is not None else None)
    h = {"content-type": ctype or "application/json"} if data is not None else {}
    req = urllib.request.Request(url, data=data, method=method, headers=h)
    try:
        r = urllib.request.urlopen(req, timeout=180)
    except urllib.error.HTTPError as e:
        r = e
    txt = r.read()
    try:
        out = json.loads(txt) if txt.strip() else None
    except ValueError:
        out = txt.decode()
    return r.status, out


def pointer(doc, p):
    for seg in [s for s in p.split("/") if s]:
        if isinstance(doc, list):
            doc = doc[int(seg)]
        elif isinstance(doc, dict) and seg in doc:
            doc = doc[seg]
        else:
            return None
    return doc


def scrub(v, ids, volatile, patterns=()):
    inv = {val: f"<{k}>" for k, val in ids.items() if isinstance(val, str) and val}
    def walk(x):
        if isinstance(x, str):
            for val in sorted(inv, key=len, reverse=True):
                x = x.replace(val, inv[val])
            x = re.sub(r"(version|seqNo|term|SeqNo) \[\d+\]", r"\1 [<n>]", x)
            for pat, rep in patterns:
                x = re.sub(pat, rep, x)
            return re.sub(r"\d{9,}", "<n>", x)
        if isinstance(x, list):
            return [walk(i) for i in x]
        if isinstance(x, dict):
            return {walk(k): ("<volatile>" if k in volatile and not isinstance(i, (dict, list)) and i not in inv else walk(i)) for k, i in x.items()}
        return x
    return walk(v)


def run(argv, name, description, steps, volatile=(), setup=(), cleanup=(), setup_note="", patterns=()):
    import argparse
    ap = argparse.ArgumentParser()
    ap.add_argument("--url", default="http://localhost:9200")
    ap.add_argument("out")
    a = ap.parse_args(argv)
    _, root = send(a.url, "GET", "/")
    v = root["version"]
    if not v["number"].startswith("3.10.0"):
        sys.exit("record: expected 3.10.0, got " + v["number"])
    for r in cleanup:
        send(a.url, r[0], r[1], r[2] if len(r) > 2 else None, query=(r[3] if len(r) > 3 else None))
    for r in setup:
        send(a.url, r[0], r[1], r[2] if len(r) > 2 else None, query=(r[3] if len(r) > 3 else None), raw=(r[4] if len(r) > 4 else None), ctype=("application/x-ndjson" if len(r) > 4 else None))
    ids, rec = {}, []
    for s in steps:
        time.sleep(s["wait"])
        path, body, query, rawb = sub(s["path"], ids), sub(s["body"], ids), sub(s["query"], ids), sub(s["raw"], ids)
        deadline = time.time() + (s["poll"][2] if s["poll"] else 0)
        while True:
            status, out = send(a.url, s["method"], path, body, query, rawb, s["ctype"])
            if not s["poll"] or pointer(out, s["poll"][0]) == s["poll"][1] or time.time() > deadline:
                break
            time.sleep(1)
        for label, p in s["capture"].items():
            val = pointer(out, p)
            if val is not None:
                ids[label] = val
        req = {"method": s["method"], "path": s["path"]}
        if s["query"]:
            req["query"] = s["query"]
        if s["body"] is not None:
            req["body"] = s["body"]
        if s["raw"] is not None:
            req["raw"] = s["raw"]
        rec.append({"name": s["name"], "request": req, "response": {"status": status, "body": scrub(out, ids, set(volatile), patterns)}})
    for r in cleanup:
        send(a.url, r[0], r[1], r[2] if len(r) > 2 else None, query=(r[3] if len(r) > 3 else None))
    doc = {"name": name, "description": description, "setup": setup_note,
           "recorded_from": {"distribution": v.get("distribution"), "version": v["number"], "build_hash": v["build_hash"]},
           "placeholders": "`{label}` in a request is the value the step that captured it received; `<label>` in a response is where that value appeared. `<volatile>` is a time, a counter or a generated value that differs on every run.",
           "steps": rec}
    json.dump(doc, open(a.out, "w"), indent=2, ensure_ascii=False)
    open(a.out, "a").write("\n")
