# Changing a tenant's cost tier or budget on a live index

## Serves

Roadmap R13.4: [tenant-tier-budgets](../tenant-tier-budgets/SPEC.md) lets a create name `index.serverless.cost.tier` and `index.serverless.cost.budget`, but an existing index could not change them: the settings update refused both as static. The shell lets an operator move a tenant between tiers on a live index. [ADR 0041](../../adr/0041-tenant-cost-levels.md).

## Change

- `PUT /{index}/_settings` accepts `index.serverless.cost.tier` (`small`, `standard` or `large`) and `index.serverless.cost.budget` (a positive whole number), both dynamic, with the create's words for a bad value. A null removes the setting.
- The change is in the descriptor as any settings update is, and the node that served the update tells its tenant levels at once, so its next tick judges the tenant against the new budget. A node that holds the shard and did not serve the update learns it when it next opens the shard, as it learns a create's.
- Not in this change: telling other nodes at once (that would take a request per index or a push), and the bucket group, which stays static.

## Acceptance criteria

- AC-1: a settings update naming a tier or budget is accepted and read back from the descriptor's settings, and a tier that is none of the three, or a budget that is 0 or text, is refused with a 400 `illegal_argument_exception`.
- AC-2: after the update the serving node's usage report shows the tenant's `budget_per_minute` as the new budget (a budget beats a tier), and after a null it shows the default tier's.
- AC-3: `index.serverless.bucket_group` is still refused as static.

## Test plan

`crates/node/src/settings_route.rs` unit tests (AC-1, AC-3) and `crates/node/tests/server/boot_usage.rs` (AC-2), each seen failing with the change removed.

## S3 request budget

Unchanged: the update is the descriptor write it already made; the levels are told in memory.

## Switch

No new switch: it rides `OSS_TENANT_LEVELS`; with levels off the settings are stored and judged by nothing.

## Risks

- Another node that holds the shard keeps its old budget until the shard next opens.
