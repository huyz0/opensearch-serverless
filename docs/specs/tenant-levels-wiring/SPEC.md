# Tenant levels wiring

## Serves

Roadmap R13.4, second piece: the policy of [tenant-levels](../tenant-levels/SPEC.md) fed by the usage ledger of [store-usage-wiring](../store-usage-wiring/SPEC.md) on a running node, and reported. [ADR 0041](../../adr/0041-tenant-cost-levels.md).

## Change

- `oss_node::tenants::TenantLevels` rolls the ledger's per-index cost up into tenants (an index's bucket group, else the index; `_node` and `_other` belong to no tenant) and steps `Guardrails` every 10 s from a thread that ends when the node's ledger is gone.
- `OSS_TENANT_LEVELS` (`off`, `observe`, `enforce`; default `observe`) and `OSS_TENANT_BUDGET_PER_MINUTE` (default 2,000 cost units, the shell's large tier, 1 to 100,000,000). Levels exist only where `OSS_STORE_USAGE` is on, which is off by default.
- `GET /_serverless/usage` adds `tenant_levels`: the mode and each tenant's level, this node's level, the level set from outside, cost and documents a minute, writes paced and refused.
- An index created with `index.serverless.bucket_group` keeps it in its descriptor's settings (flat JSON, which the Java reads), and a shard opened on a writer node tells the levels its group, so the indices of a group are one tenant. A node that has not opened a shard of an index yet shows it as its own tenant; reader-only nodes do not learn groups.
- `OSS_TENANT_TICK_MS` (default 10,000; 10 to 3,600,000) sets how often the levels step.
- Not in this change: budgets by tier and per index, the documents counter, and everything levels act on (publish stretch, search lane, write pacing, 429), so `enforce` computes the same as `observe`.

## Acceptance criteria

- AC-1: two indices noted into one group are one tenant whose cost is their sum; an index with no group is its own tenant.
- AC-2: a tenant over its budget for a minute is at level 1; one under it stays at 0.
- AC-3: noting an empty group, or the index's own name, forgets the group.
- AC-4: with the mode `off` nothing is judged and nothing is reported.
- AC-5: the usage route reports the mode and each tenant's levels and rates.
- AC-6: the mode defaults to `observe` and the budget to 2,000; an unknown mode and a budget of 0 are refused.
- AC-7: the ticker steps the levels on its own.
- AC-8: on a booted node, an index created with a bucket group and written to is reported under the group, an index with none under its own name, and the index name of the first not at all.
- AC-9: a group written to descriptor settings reads back, flat or nested, including quotes and non-ASCII; settings with none, an empty one or text that is no JSON read as none.

## Test plan

`crates/node/tests/tenants.rs` (AC-1 to AC-5, AC-7, AC-9, AC-1 seen failing with the roll-up by index) and `tenant_levels_observe_with_a_large_budget_unless_set` in the config tests (AC-6) and `boot_usage.rs` (AC-8).

## S3 request budget

Unchanged: the tick reads the in-memory ledger; no request.

## Switch

`OSS_TENANT_LEVELS`, default `observe`: it computes and reports and slows nothing, and costs one pass over at most `OSS_STORE_USAGE_MAX_INDICES` ledger lines every 10 s. It only runs with `OSS_STORE_USAGE` on.

## Risks

- Until a writer node opens a shard of an index it shows as its own tenant, against the whole budget; indices created before this change carry no group.
- The ticker thread ends within a second of the ledger's last other holder dropping; a test node leaves it for that long.
