# Tenant publish stretch

## Serves

Roadmap R13.4, the first lever: at level 1 a tenant's writes wait four times longer to be published, as in the shell's `Guardrails`. Builds on [tenant-levels-wiring](../tenant-levels-wiring/SPEC.md); [ADR 0041](../../adr/0041-tenant-cost-levels.md).

## Change

- `LazyPublish::due_stretched` multiplies the age a write may wait by a stretch (1 changes nothing); `due` is a stretch of 1. The operation count still bounds the wait, and a shard never published is still published at once.
- A writer role takes a stretch by index (`set_publish_stretch`); the publish timer asks it for each shard it considers.
- `TenantLevels::publish_stretch` answers 4 for an index whose tenant is at level 1 or above when the mode is `enforce`, else 1. `observe` and `off` never stretch. A level set from outside (`TenantLevels::set_levels`) counts.
- Not in this change: how a collector reaches `set_levels` (a cluster setting), and the other levers.

## Acceptance criteria

- AC-1: with a stretch of 4 a shard with a 1 s age is due after 4 s, not before; with 1 it is due after 1 s; `due` is the same as a stretch of 1.
- AC-2: an enforced tenant at level 1 answers 4, an observed or off one 1, an unrelated index 1, and a tenant with no level 1.
- AC-3: a writer role asked for a stretch by index name applies it to that index's shards only, and takes one stretch.

## Test plan

`a_stretched_age_waits_that_many_times_longer_and_one_changes_nothing` (writer crate, seen failing with the multiplier removed) and `an_enforced_tenant_at_level_one_publishes_four_times_less_often` (`crates/node/tests/tenants.rs`), and `a_stretch_asked_by_index_name_lengthens_the_wait_of_that_index` (`crates/node/tests/roles/writer_publish.rs`).

## S3 request budget

Fewer: a stretched tenant publishes less often. No request is added.

## Switch

`OSS_TENANT_LEVELS=enforce`; the default `observe` changes nothing, and levels exist only with `OSS_STORE_USAGE` on.

## Risks

- A longer wait is a longer time before a refresh-less write is searchable from a commit; writes a search needs are still pushed by the tail overlay, and a refresh publishes at once.
