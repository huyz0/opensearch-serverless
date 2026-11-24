# 0041: A tenant's cost is held to a budget by levels, in observe mode unless set to enforce

**Status:** Accepted after independent review of the change, 2026-11-24

## Context

One tenant's work can cost the object store more than its neighbours', and the bill is requests ([ADR 0037](0037-per-index-store-usage.md) counts them per index). The shell holds a tenant to a budget with levels, decided on 2026-11-14: a tenant is its bucket group or its index; cost units are a PUT 1, a GET 0.08; budgets by tier (small 20, standard 200, large 2,000 a minute, burst 4x); observe mode first.

## Decision

- Levels follow the shell's: a minute over for level 1, two more minutes for each next level to 3, five minutes under to come down one; a level set from outside holds when higher; a refusal shows as 4 for a minute.
- The policy is sans-IO in `oss_blob::guard`: time, budgets and cumulative cost come from the caller, so it is tested with a clock that is an integer and moves nowhere on its own.
- What a level does (stretch publishes, wait for search slots, pace writes, refuse) acts only in enforce mode and only on that tenant's indices. The default stays observe: levels are computed and reported, and nothing slows a tenant.
- Pacing never touches the store or another node: it is a token bucket held in memory.

## Consequences

- No request is added at any level; a level 1 stretch removes publishes, which are PUTs.
- A tenant is seen per node; an every-node level comes from a collector that sets `tenant=level` pairs.
- The wiring (tenant from settings, budgets by tier, timer, hooks in the write, publish and search paths) follows in separate changes, each behind the mode.
