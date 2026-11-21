# Fault suites for the proactive takeover

## Serves

Parity plan R8.6 and [ADR 0021](../../adr/0021-proactive-takeover-from-the-node-log.md): the switch `serverless.takeover.proactive.enabled` stays off until fault suites show write p99 within 5%, no extra requests on any existing path, no search regression and the crash tests passing. Builds on [proactive-takeover](../proactive-takeover/SPEC.md); its AC-6, AC-7, AC-9, AC-12, AC-13 and AC-20 are held here.

## Change

- Tests only, over the in-process emulator with injected latency and failures: `crates/node/tests/proactive_faults.rs` (faults), `proactive_evidence.rs` (cost of the switch) and a shared rig, `proactive_support/`.
- Found by those tests and fixed with them:
  - A shard a live node holds counted as a deferred take, so the pass ran again on every refresh. It is now skipped; only a failed take is retried.
  - The node never told its write log that a shard published, so the log never trimmed and a header named every shard the node ever wrote. A publish now records the position the shard's records reached (taken under the shard's lock, so the commit holds them all).
  - The 8 MiB header cap of the ADR did not exist. `NodeLogTakeover::named_within` refuses a header over a cap from its prefix; the pass records that dead node's epochs as finished (its shards stay lazy) instead of reading the header on every refresh.
  - The pass read one descriptor per named shard; it reads one per index.
  - A dead lease naming no epochs started a pass.
  - The dead-log pass fenced a log the proactive pass had already fenced. Both now read it through the node's shared `Predecessors` (`DeadLogParts::predecessors`), and a collected log is forgotten.
  - `Predecessors` kept a dead node's log for the process's life: a node that died, came back under the same id and died again was read by its old epochs. `of_naming` rebuilds the reader when the epochs differ.
- Not changed: the head, log and register formats, lazy takeover, the switch's default (off).

## Acceptance criteria

- AC-1: with the switch on and nothing dead, or a dead lease naming no epochs, a refresh makes no request.
- AC-2: one pass takes every shard the dead node's last header names.
- AC-3: a shard published before the last blob is not named and not touched.
- AC-4: a take that fails (every PUT refused after the fence) holds nothing, is retried by a later refresh, and the next write takes the shard lazily with nothing lost.
- AC-5: a take whose answer was lost is finished by the next pass.
- AC-6: two reapers racing over four shards leave each shard held once, over eight rounds.
- AC-7: a shard a live node holds is skipped, and the pass is not run again for it.
- AC-8: an index recreated under another uuid is skipped.
- AC-9: past `pass_millis` no new take starts and the unstarted shards stay due.
- AC-10: no more shards are taken at once than `max_concurrent` (a time floor of requests in sequence).
- AC-11: a header over the cap is refused, nothing is taken, the pass is not run again, and each shard is still taken by its first write.
- AC-12: serving four shards by a pass then a write each costs at most one request more than the four writes alone (one descriptor read), and no LIST.
- AC-14: the dead-log pass after a proactive pass reuses its fence: collecting costs fewer requests than through its own reader (15 against 8 measured), and still collects.
- AC-15: a log read by `Predecessors` is kept while the lease names the same epochs (a later trim point included), rebuilt when it names others, and forgotten on request.
- AC-16: a write placed on a node that does not hold the shard goes to the reaper that took it, without taking it, and the next costs one request (the index lookup).
- AC-17: a pass costs the request-budget rows `takeover.proactive.*`: 19 for one named shard, 61 for four, no LIST.
- AC-18: a reader opens the shard after a pass and sees the documents a lazy take publishes, with no write to ask for the take.
- AC-13: a live node's write p99 with a pass running is within 5% of without, at S3-like latency (run by hand, `--ignored`).

## Test plan

As named above; each criterion is a test in the files listed. AC-13 is a percentile, so it is evidence recorded in VERIFIED.md, not a gate.

## S3 request budget

The pass adds one descriptor GET per index a header names, once per dead node; everything else is what lazy takes cost. No LIST. A publish now trims the log in memory and adds no request.

## Switch

None added; `serverless.takeover.proactive.enabled` is unchanged and still off.

## Risks

- The revoked-versus-expired distinction of the refresh surface (proactive-takeover AC-19, second half) is not built; nothing reads it yet.
