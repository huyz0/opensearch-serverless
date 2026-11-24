# Tenant tier budgets

## Serves

Roadmap R13.4: the budget a tenant's cost is judged against, by tier, as in the shell's `Guardrails` (small 20, standard 200, large 2,000 cost units a minute). Builds on [tenant-levels-wiring](../tenant-levels-wiring/SPEC.md); [ADR 0041](../../adr/0041-tenant-cost-levels.md).

## Change

- An index can be created with `index.serverless.cost.tier` (`small`, `standard` or `large`) and `index.serverless.cost.budget` (cost units a minute, a positive whole number; it wins over the tier). Both are kept in the descriptor's settings next to the bucket group, and a writer node that opens a shard of the index tells the levels. Any other index setting is still refused at create.
- Node settings: `OSS_TENANT_BUDGET_SMALL` (20), `OSS_TENANT_BUDGET_STANDARD` (200), `OSS_TENANT_BUDGET_PER_MINUTE` (2,000, the large tier), and `OSS_TENANT_DEFAULT_TIER` (`small`, `standard`, `large`; unset it is `large` in `observe`, so no one is flagged before budgets are chosen, and `standard` in `enforce`).
- A tenant's budget is the largest of its indices' budgets (an index with none has the default tier's). The tick judges each tenant against it, and the usage report shows it as `budget_per_minute`.
- Not in this change: changing tier or budget of an existing index (the node has no index settings update yet), a cluster setting for the tier budgets.

## Acceptance criteria

- AC-1: a create naming a tier or a budget keeps them, and a bad tier, a budget of 0 or text, is refused with a 400.
- AC-2: tier and budget written to descriptor settings read back; a budget beats a tier; settings with neither read as none.
- AC-3: a tenant's budget is the largest of its indices'; an index with none takes the default tier's; the default tier is `large` in observe and `standard` in enforce unless set.
- AC-4: a tenant judged against a smaller budget reaches level 1 where one against a larger budget stays at 0, at the same cost, and the report names it.
- AC-5: the usage report names each tenant's budget.
- AC-6: the node settings default to 20, 200, 2,000 and the mode's tier; an unknown tier, or a budget of 0, is refused.

## Test plan

`crates/node/tests/tenants.rs` (AC-2 to AC-5), `crates/node/tests/server/index_route.rs` (AC-1), the config tests (AC-6), each seen failing with the behaviour removed.

## S3 request budget

Unchanged: the settings ride in the descriptor create already writes.

## Switch

Rides `OSS_TENANT_LEVELS`: in `observe` the budgets only decide which level is reported.

## Risks

- A tenant of one big index and many small ones is judged against the big one's budget (the largest wins), so a noisy small index hides behind it.
- A tier cannot change until the node has an index settings update.
