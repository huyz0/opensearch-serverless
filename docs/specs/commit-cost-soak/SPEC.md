# Commit staging and logged commits: the cost soak

Status: measured on the in-process emulator with latency; provisional.

## Serves

Parity plan R11.5, the measurement that decides whether the commit switches ([commit-staging](../commit-staging/SPEC.md), [logged-commits](../logged-commits/SPEC.md), [open-from-logged-commit](../open-from-logged-commit/SPEC.md)) may become defaults. The build rule: on by default only if write p99 is within 5%, no extra S3 requests, no search regression and the crash tests pass. This pins what a node and a takeover ask of the store, and what a write costs, with the switches off, with staging, and with staging, logging and opening from logged commits.

## Acceptance criteria

- AC-1: with staging on, a node taking continuous writes without refresh to 6 of 24 one-shard indices for about ten seconds and then idling makes fewer PUTs than with it off, and no more requests in total than a tenth over. (An explicit refresh publishes in full, so refresh=true writes show no difference: staging is for the background pass.)
- AC-2: with staging, logging and opening on, the same phase makes no more requests in total than staging alone, within a tenth.
- AC-3: write latency at the 99th percentile with each switch set is within 5% of the switches off.
- AC-4: a second node taking over the 24 indices after the first is killed keeps every document written before and after, including one acknowledged just before the kill that only the log holds; with staging it makes at most 15% more requests than with the switches off, and opening from the logged commit makes no more than staging alone. (The build rule asks for no extra requests; this bound pins how much more it is, and the Decision applies the rule.)
- AC-5: a search of a taken-over index finds both documents (no search regression).

## Measured (6 hot indices of 24, one node, emulator with a latency profile; provisional)

Two runs of `commit_cost.rs`, requests of the steady phase (about 10 s of continuous writes and 4 s idle) and of the takeover (the second node writing to all 24 indices after the first was killed, a document to every index acknowledged just before the kill and not yet published):

| | Switches off | Staging | Staging, logging, opening |
|---|---|---|---|
| steady, run 1 | 57 GET, 140 PUT, 9 DELETE (206) | 57 GET, 111 PUT, 1 DELETE (169) | 60 GET, 112 PUT, 1 DELETE (173) |
| steady, run 2 | 57 GET, 142 PUT, 10 DELETE (209) | 57 GET, 111 PUT, 1 DELETE (169) | 63 GET, 113 PUT, 1 DELETE (177) |
| takeover, run 1 | 377 GET, 90 range, 2 HEAD, 140 PUT, 5 DELETE (614) | 382 GET, 108 range, 2 HEAD, 176 PUT, 5 DELETE (673) | 335 GET, 122 range, 4 HEAD, 174 PUT, 5 DELETE (640) |
| takeover, run 2 | 373 GET, 90 range, 2 HEAD, 138 PUT, 6 DELETE (609) | 380 GET, 109 range, 2 HEAD, 175 PUT, 5 DELETE (671) | 335 GET, 122 range, 4 HEAD, 176 PUT, 5 DELETE (642) |
| write p99 (96 and 240 writes) | 256 and 270 ms | 256 and 257 ms | 256 and 257 ms |
| takeover time | 9.5 s | 9.1 s | 8.9 to 9.0 s |

Staging saves 18% of requests (21% of PUTs) in the steady phase when a shard is published several times between checkpoints, and its cost is a takeover that makes 10% more requests and 26% more PUTs (the successor replays what the register lacks and publishes it). Logging adds about 3 requests to the steady phase and, with opening from the logged commit, brings the takeover to 5% above off in total, though not in PUTs (24% above). Write latency is level.

## Decision

By the build rule, a default is on only if there is no extra S3 request, and measured against it:

- Write p99 is within 5%: pass for all three.
- No search regression: pass (every index answered with every document after the takeover).
- Crash: no acknowledged write lost in any run: pass for the kill after an acknowledged unpublished write.
- No extra requests: **fails for the takeover** (staging +10% total, +26% PUTs; logged +5% total, +24% PUTs), while the steady phase saves 18%. Whether that trade wins "in most cases" (non-negotiable on features with a measured downside) depends on how often a node is taken over against how hot its shards are: over this load, 59 requests more at one takeover against 37 fewer per ten seconds of 6 hot shards.

None of the three switches is flipped here: staging, logging and opening stay off by default, as the owner's decision. The numbers are for the owner to decide the default; a longer checkpoint age than the 10 s used here saves more in the steady phase and costs more at a takeover.

## What this does not measure

A cluster-wide runtime override of the checkpoint age; Java's +23% takeover figure as a ratio (the Java shell is not run here); a node lost after the adoption swap and before the head is settled, a replay long enough to cross the pack deadline, and 1,000 shards (this is 24 indices: the heap and the per-publish counts at 1,000 are in the writer soak).
