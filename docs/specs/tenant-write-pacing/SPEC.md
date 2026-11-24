# Tenant write pacing

## Serves

Roadmap R13.4, the write lever: at level 3 a tenant's writes are held to what its budget allows, and refused with a 429 past the longest wait, as in the shell's `Guardrails`. Builds on [tenant-levels](../tenant-levels/SPEC.md) (the policy) and [tenant-levels-wiring](../tenant-levels-wiring/SPEC.md); [ADR 0041](../../adr/0041-tenant-cost-levels.md).

## Change

- A writer role takes a write gate (`set_write_gate`): the index name and the documents of a group in, a wait in milliseconds or a refusal out. `admit_group` asks it before the group is applied, outside every lock: it sleeps the wait, or fails every item of the group with the throttle outcome that already answers 429 with a `Retry-After`.
- `TenantLevels::admit_write` counts the documents (every mode, so the rate shows in the report and feeds the tick) and asks `Guardrails::pace_write`, which holds a write only for an enforced tenant at level 3 over its budget. `observe` and `off` never hold or refuse.
- The tick passes each tenant's documents to the policy, so the budget converts to documents a second at the ratio the tenant writes at.
- A refusal's `Retry-After` is the longer of the limiter's and the wait the tenant would have had, in whole seconds (at least 1); a write the gate lets through clears it.
- Not in this change: the search lane (the node has no search slots yet), tier budgets and a cost header.

## Acceptance criteria

- AC-1: documents of every write are counted per tenant and appear as `docs_per_minute` in the usage report after two ticks.
- AC-2: an enforced tenant at level 3 over budget has a write within its burst admitted at once, a write past it held for the wait, and one that would wait past the longest wait refused; in `observe` the same tenant is never held or refused.
- AC-3: a writer role with a gate that answers a wait sleeps it before applying; one that refuses fails each item of the group as throttled and applies nothing.
- AC-4: the retry-after a refusal carries is at least a second and covers the wait; a later admitted write clears it.

## Test plan

`crates/node/tests/tenants.rs` (AC-1, AC-2) and `crates/node/tests/roles/write_gate.rs` (AC-3, AC-4), each seen failing with the behaviour removed.

## S3 request budget

Fewer: a paced tenant writes fewer documents a second, so the node log takes fewer PUTs. No request is added.

## Switch

`OSS_TENANT_LEVELS=enforce`; the default `observe` counts documents and changes nothing for the writer.

## Risks

- A wait holds a request thread up to the longest wait (5 s) before the write; the thread holds no lock.
- Documents are counted per group before they are applied; a group the limiter then refuses is still counted.
