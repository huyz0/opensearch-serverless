#!/usr/bin/env python3
"""Fail-closed gate for a `ci` run that checks only the PR text.

  ci-code-run.py REPO HEAD_SHA SELF_RUN_ID   wait for, then require, a green code run
  ci-code-run.py --self-test                 prove the decision on fixed examples

A description or label edit runs `ci` in text-only mode (about 20 s, no build).
That run reports a check named `ci` on the same commit as the code run, so a
green text run beside a cancelled or failed code run would read as a pass. This
script makes the text run pass only if another `ci` run on the head commit
finished its last step ("code checked", which runs only when every step before
it passed): it waits while such a run is still going, and fails if none passed.
"""
import json
import subprocess
import sys
import time

MARKER = "code checked"
WAIT_SECONDS = 20 * 60
POLL_SECONDS = 30


def decide(runs):
    """runs: [{status, checked}] for every other `ci` run on the commit.
    Returns "pass", "wait" or "fail"."""
    if any(r["checked"] for r in runs):
        return "pass"
    if any(r["status"] != "completed" for r in runs):
        return "wait"
    return "fail"


def gh(path):
    out = subprocess.run(["gh", "api", path], capture_output=True, text=True, check=True).stdout
    return json.loads(out)


def other_runs(repo, sha, self_id):
    listing = gh(f"repos/{repo}/actions/workflows/ci.yml/runs?head_sha={sha}&event=pull_request&per_page=50")
    runs = []
    for run in listing["workflow_runs"]:
        if run["id"] == self_id:
            continue
        checked = False
        if run["status"] == "completed":
            jobs = gh(f"repos/{repo}/actions/runs/{run['id']}/jobs")["jobs"]
            checked = any(s["name"] == MARKER and s["conclusion"] == "success"
                          for j in jobs for s in j["steps"])
        runs.append({"status": run["status"], "checked": checked})
    return runs


def self_test():
    done_ok, done_bad, going = {"status": "completed", "checked": True}, {"status": "completed", "checked": False}, {"status": "in_progress", "checked": False}
    cases = [
        ([], "fail"),
        ([done_bad], "fail"),
        ([done_bad, done_bad], "fail"),
        ([going], "wait"),
        ([done_bad, going], "wait"),
        ([done_ok], "pass"),
        ([done_bad, done_ok], "pass"),
        ([going, done_ok], "pass"),
    ]
    bad = [f"{runs} -> {got}, expected {want}" for runs, want in cases if (got := decide(runs)) != want]
    print("\n".join(bad) if bad else f"ci-code-run: self-test ok ({len(cases)} cases)")
    return 1 if bad else 0


def main():
    if "--self-test" in sys.argv:
        return self_test()
    repo, sha, self_id = sys.argv[1], sys.argv[2], int(sys.argv[3])
    deadline = time.monotonic() + WAIT_SECONDS
    while True:
        verdict = decide(other_runs(repo, sha, self_id))
        if verdict == "pass":
            print(f"a code run on {sha[:12]} passed every step")
            return 0
        if verdict == "fail":
            print(f"::error::no `ci` run on {sha[:12]} finished its checks (cancelled, failed or never started): re-run the code run, this text run proves nothing")
            return 1
        if time.monotonic() > deadline:
            print("::error::the code run is still going after 20 minutes")
            return 1
        print("code run still going; waiting")
        time.sleep(POLL_SECONDS)


if __name__ == "__main__":
    sys.exit(main())
