#!/usr/bin/env python3
"""A Markdown report of CI cost over the last seven days.

  ci-report.py OWNER/REPO     (needs `gh` signed in, or GH_TOKEN)

Per workflow: runs, median and worst queue wait (created to started), and
median and total run minutes; then the ten slowest jobs. Queue wait matters
here because the free runner pool is shared by all of the owner's repos.
"""
import datetime as dt
import json
import statistics
import subprocess
import sys


def gh(path):
    out = subprocess.run(["gh", "api", "--paginate", path], capture_output=True, text=True, check=True).stdout
    pages, decoder, i = [], json.JSONDecoder(), 0
    while i < len(out):
        obj, i = decoder.raw_decode(out, i)
        pages.append(obj)
        while i < len(out) and out[i].isspace():
            i += 1
    return pages


def ts(s):
    return dt.datetime.fromisoformat(s.replace("Z", "+00:00")) if s else None


def minutes(a, b):
    return (b - a).total_seconds() / 60 if a and b else None


def main():
    repo = sys.argv[1]
    since = (dt.datetime.now(dt.timezone.utc) - dt.timedelta(days=7)).date().isoformat()
    runs = [r for p in gh(f"repos/{repo}/actions/runs?created=>={since}&per_page=100") for r in p["workflow_runs"]]
    by_wf, jobs = {}, []
    for r in runs:
        if r["status"] != "completed":
            continue
        wait = minutes(ts(r["created_at"]), ts(r.get("run_started_at")))
        dur = minutes(ts(r.get("run_started_at")), ts(r["updated_at"]))
        by_wf.setdefault(r["name"], []).append((wait or 0, dur or 0))
        for p in gh(f"repos/{repo}/actions/runs/{r['id']}/jobs?per_page=100"):
            for j in p["jobs"]:
                d = minutes(ts(j.get("started_at")), ts(j.get("completed_at")))
                if d is not None:
                    jobs.append((d, r["name"], j["name"], r["html_url"]))
    print(f"# CI report since {since}\n")
    print("| Workflow | Runs | Queue median | Queue worst | Run median | Run total |")
    print("|---|---|---|---|---|---|")
    for name, rows in sorted(by_wf.items()):
        waits, durs = [w for w, _ in rows], [d for _, d in rows]
        print(f"| {name} | {len(rows)} | {statistics.median(waits):.1f} min | {max(waits):.1f} min | "
              f"{statistics.median(durs):.1f} min | {sum(durs):.0f} min |")
    print("\n## Slowest jobs\n")
    for d, wf, job, url in sorted(jobs, reverse=True)[:10]:
        print(f"- {d:.1f} min: {wf} / {job} ([run]({url}))")
    return 0


if __name__ == "__main__":
    sys.exit(main())
