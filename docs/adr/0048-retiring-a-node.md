# 0048: A node told to stop publishes its shards and gives their heads back, within a deadline

**Status:** Accepted after independent review (three rounds), 2026-11-24

## Context

A node that stops today lets its member lease lapse and its shard heads keep naming it. Other nodes then wait out the lease lifetime (30 s by default) before they may take its shards, and a take after a lapse revokes the old holder's liveness, so every rolling restart or scale-in stalls every shard the node held. A hibernated shard already gives its head back with two requests ([ADR 0029](0029-giving-back-a-hibernated-head.md)); what is missing is doing that for every held shard when the node is told to stop.

## Decision

- `Node::retire(timeout)` stops serving first (the HTTP server is shut down, so every in-flight request finishes and none starts; its timers go with it, so the publish and hibernation timers cannot run alongside), then publishes each shard that owes a commit, puts it to sleep and gives its head back through the existing hibernation and give-back paths, then releases the member lease. The sleep step is the hibernation pass at an idle time of 0: a shard is slept only if, under its own slot lock, nothing is unpublished and nothing reached it meanwhile.
- The work is bounded by `OSS_RETIRE_TIMEOUT_MS` (default 25,000, under the usual 30 s termination grace; 0 skips the shards and only stops the node and releases the lease). The deadline is checked before each shard and each give-back pass; shards not reached are counted `unfinished`. The passes loop until no asleep record is left; a pass takes the record of every shard it examines, so the loop ends.
- A shard that fails to publish, is written to meanwhile, is pinned, or whose head moved keeps its head and is left to the lease. Releasing the member lease is still safe: a successor then takes those heads at once and recovers any unpublished writes from the dead node log, the way it does after a crash.
- The retire runs on its own thread in `main`, which waits for the timeout plus a 5 s grace (`retire_within`): a blocking store call or a request that never finishes cannot hold the process, which then exits 1 and leaves the shards to the lease. The deadline is counted from before the server's shutdown and checked per shard, inside a give-back pass too.
- A shard left held (busy, failed, unfinished) loses no acknowledged write: a write is acknowledged only once the node log blob holding it landed, and a successor replays the dead node's log when it takes the head, as after a crash. That is why exit 0 with busy shards is correct. The exit 130 of a second signal skips the report and the lease release; the lease then lapses. Non-Unix builds have no signal wait, so `retire` is reachable only through the library there.
- `main` calls it on `SIGTERM` and `SIGINT` using the `signal-hook` crate's safe iterator (Unix; no `unsafe` here). A second signal while retiring ends the process at once with status 130. A handler that cannot be installed is a start failure. A signal during `start` takes the default action.
- Exit status: 0 retired or left to the lease; 1 a shard failed to publish or the deadline passed first; 2 a configuration error.

## Switch

The setting above is the switch. Default on, because a planned stop that waits out a lease lifetime stalls every held shard for 30 s and then costs the successor a revocation, while the retire costs a bounded number of requests once. Measured on the emulator (provisional): one shard with one unpublished write costs 6 requests (`writer.retire.one_shard.requests`); the lapse path costs none at the stop and the take's extra requests later. Downsides weighed: a stop can take up to the deadline; a shard given back and then written pays a take (about 14 requests) instead of a wake (about 5), which is why ADR 0029 keeps its own timer-driven give-back off by default, but here the node is leaving, so there is no wake to compare with; clients see connection refusals from the server's shutdown to the process end, which a readiness endpoint (a later piece) would cover. Requests scale with the shards held at a stop, once, not with nodes and bytes: the accepted exception, bounded by the deadline, none while running.

## Consequences

- A planned stop costs per held shard what a refresh and a give-back already cost; an unplanned one costs nothing and waits a lease lifetime, unchanged.
- Takeover after a planned stop needs no lease revocation: the next node takes the unowned head at once.
- Pinned shards are held and counted busy; they are not given back.
