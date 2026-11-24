# 0048: A node told to stop publishes its shards and gives their heads back

**Status:** Proposed

## Context

A node that stops today lets its member lease lapse and its shard heads keep naming it. Other nodes then wait out the lease lifetime (30 s by default) before they may take its shards, so every rolling restart or scale-in is a stall on every shard the node held. A hibernated shard already gives its head back with two requests ([ADR 0029](0029-giving-back-a-hibernated-head.md)); what is missing is doing that for every held shard when the node is told to stop.

## Decision

- `Node::retire` stops serving first (HTTP server and timers), so nothing wakes a shard, then publishes each shard that owes a commit, puts it to sleep and gives its head back through the existing hibernation and give-back paths, and releases the member lease.
- `main` calls it on `SIGTERM` and `SIGINT`, using the `signal-hook` crate's safe iterator (no `unsafe` here).
- A shard written to during the retire stays held and is left to the lease, as a crash would leave it. A shard that cannot publish keeps its head, so no acknowledged write is made unreachable; the exit code says so.
- No setting: it adds no request while running and happens once per stop. A killed process behaves as before.

## Consequences

- A planned stop costs per held shard what a refresh and a give-back already cost (about 4 requests); an unplanned one costs nothing and waits a lease lifetime, unchanged.
- Takeover after a planned stop needs no lease revocation: the next node takes the unowned head at once.
- A node that is being retired is not a member for the window between the server stopping and the lease release; clients behind a load balancer see connection refusals for that window, which a readiness endpoint (a later piece) would cover.
