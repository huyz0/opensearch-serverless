# Usage report `?top=N`

## Serves

Roadmap R13.3/R13.4: with thousands of indices a node's usage report is large; an operator looking for who costs most wants the first few. Builds on [tenant-levels-wiring](../tenant-levels-wiring/SPEC.md).

## Change

- `GET /_serverless/usage?top=N` lists the `N` indices that cost most (cost units, highest first, ties by name) and, when tenant levels are reported, the `N` tenants with the highest cost a minute. `_node` and `_other` are always listed. Without `top` the report is whole, as before.
- `top` is a whole number of at least 1; anything else is a 400.

## Acceptance criteria

- AC-1: with `top=N` the report names the `N` costliest indices, highest first, plus `_node` and `_other`, and no other index.
- AC-2: with `top=N` the report names the `N` tenants with the highest cost a minute.
- AC-3: without `top` every index and tenant is named.
- AC-4: `top=0`, a negative number or text is refused with a 400.

## Test plan

`crates/node/tests/usage_top.rs` over a ledger and the tenant levels, each seen failing with the behaviour removed.

## S3 request budget

Unchanged: the report reads counters in memory.
