# Proactive takeover on by default

## Serves

[ADR 0021](../../adr/0021-proactive-takeover-from-the-node-log.md): the switch flips in a PR of its own once the R8.6 fault suites ([proactive-takeover-faults](../proactive-takeover-faults/SPEC.md)) show write p99 within 5%, no extra requests on any existing path, no search regression and the crash tests passing.

## Change

- `serverless.takeover.proactive.enabled` and `OSS_PROACTIVE_TAKEOVER` default to `true`. Setting either to `false` keeps today's behaviour, pinned by the existing off-case tests.

## Acceptance criteria

- AC-1: a node configured with no proactive setting boots with the pass on.
- AC-2: a node with `OSS_PROACTIVE_TAKEOVER=false` leaves a dead node's shard untaken until a write asks.
- AC-3: with the switch on and nothing dead, a refresh makes no request.
- AC-4: the proactive-takeover-faults criteria hold (crash, race, stale, deadline, cap, routing, budget).

## S3 request budget

Unchanged by the default itself; the pass's rows are `takeover.proactive.*` in [request-budgets](../../request-budgets.md). Against lazy takes it adds one descriptor GET per death and no LIST; it ends the recurring read of a dead lease that names epochs.

## Switch

The existing one, now default on, overridable per node.

## Risks

- Measured on the in-process emulator and not on RustFS: the write p99 ratio (0.996) is provisional.
- A reaper fences a node that was only partitioned; it loses its epoch and opens a new one, as with any takeover.
- A pass pays for shards a lazy take might never have been asked for; bounded by the header's named shards, `max_concurrent` and the deadline.
