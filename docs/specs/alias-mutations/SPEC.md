# Alias mutations

## Serves

The plan's metadata-writes milestone, seventh part. An index's descriptor lists the aliases that name it (`aliased_by`), so deleting an index finds its aliases without enumerating every alias. That hint is only useful if it never misses an alias that names the index, even when a writer crashes halfway through a change. Follows [alias-records](../alias-records/SPEC.md).

Invariants served:
- **I-1:** every store call is metered.
- **Fencing:** every step is a compare-and-swap, conditional create or conditional delete. There is no lock or lease: a reservation's deadline only says when another writer may undo it.
- **Requests scale with nodes, not indexes:** a mutation costs requests per member it adds or removes, never per member it keeps. Finding an index's aliases reads its hint's names, never every alias.

## Change

Decision record: [ADR 0008](../../adr/0008-metadata-crate.md) (Accepted). The bytes are the Java's, so Rust and Java writers can recover each other's mutations.

Ported from huyz0/OpenSearch at `3920dc36`:
- `serverless/shell/src/main/java/org/opensearch/serverless/metadata/AliasMutations.java`: `createAlias`, the three-argument `updateAlias`, `deleteAlias` with a generation, `detachFromAliases`, `recoverAlias`, `resolveForAliasMutation`, `aliasesOf`, and the hint steps.
- `cluster/AliasRecord.java`: `preparing`, `published`, `withMembers`, `mutationUntil`, `pendingAdditions`.

### The protocol

A change runs in three steps:

1. **Reserve.** The alias register is swapped, or created, with a pending mutation. The mutation names the members it adds and a deadline: now plus the lease TTL.
2. **Prepare.** Each member the mutation adds gains the alias in its descriptor's `aliased_by`. A member it keeps is not read: its hint stays, because every removal reads the alias after the descriptor, and the reservation still names the member. The Java reads every member, which would make adding one backing index to a data stream cost two reads for each index it already has. Each swap first re-reads the reservation, and the change stops with `Superseded` if the reservation moved. A member it adds that is already hinted is swapped again, so a removal that read the descriptor earlier loses.
3. **Publish.** The alias is swapped to its new membership. Then the hints of members it dropped are removed, unless the alias, read after the descriptor, still protects them.

A failed step, or a lost publish, aborts:
- a published record is restored, or a creation that never published is deleted;
- the hints the mutation added are removed, each descriptor swapped even when its hint is already gone, which fences a late preparation.

A writer that finds a reservation whose deadline is not after now aborts it before reading. `resolve_for_mutation` waits up to 255 ms for a live one, in pauses doubling from 1 ms to 64 ms. A record still pending after the wait is returned, and a change from it recovers rather than applies. A creation still pending reads as no alias.

### In `oss-meta`

- `AliasMutations::new(descriptors, clock, lease_ttl_millis, register_bytes)` owns three register buffers, allocated once. `AliasClock` carries the time and the pause, so tests run them by hand.
- `create_alias`, `update_alias(before, alias, expected)` and `delete_alias(alias, expected)` follow the protocol. An update that would rename the alias is refused before any request. `MetaError::MissingMember` reports a member that is no index; `MetaError::Superseded` reports a lost reservation.
- `detach_from_aliases(index, aliased_by)` drops a deleted index from each alias that names it. An alias left with no other index keeps naming it, so a search through the alias says the index is gone. A live mutation that protects the index is aborted first. It returns how many aliases kept changing through four attempts, where the Java logs a warning. Unlike the Java, it does not count an alias it rightly left alone.
- `aliases_of(index, aliased_by)` confirms the hint: one read for each name in it.

### Not in this change

- The Java marks an alias in the search digest rollups (`DigestRollups.markAlias`); those are not ported yet.
- The REST `_aliases` actions, and data stream rollover, call this later.

## Acceptance criteria

- AC-1: Creating an alias hints every member before it publishes, at an exact request count on each backend. A member that is no index undoes the creation and frees the name. An update hints the members it adds and unhints the ones it removes, at an exact count; a stale one costs only the refused swap. A kept member costs nothing. A data stream keeps its fields. Deleting an alias unhints its members, and a stale delete changes nothing, each at an exact count. An update that renames is refused before any request. Tests: `creating_an_alias_hints_every_member_before_it_is_published`, `a_member_that_is_no_index_undoes_the_creation`, `an_update_hints_added_members_and_unhints_removed_ones`, `a_data_stream_keeps_its_fields_through_a_mutation`, `deleting_an_alias_unhints_its_members`, `an_update_cannot_rename_an_alias`.
- AC-2: A crashed writer's mutation is recovered once its deadline is not after now: a reserved update is restored without the hints it added, and an abandoned creation is deleted, each at an exact count. A live reservation is waited for in the Java's pauses and then left alone, and a creation still pending reads as no alias. The deadline is now plus the TTL, saturating, and above zero at the epoch. Tests: `an_expired_reservation_is_recovered_by_the_next_writer`, `an_abandoned_creation_is_deleted_by_the_next_writer`, `a_live_reservation_is_waited_for_then_left_alone`, `a_creation_still_pending_after_the_wait_reads_as_no_alias`, `a_reservation_carries_its_deadline`, `a_reservation_deadline_saturates`.
- AC-3: Races: a reservation removed, or swapped by another writer, during preparation stops the change with `Superseded`, and its abort fences the members. A failed preparation or store failure undoes the reservation. A hint is kept while the alias, read after the descriptor, names its index again. An addition already hinted is swapped again, so a removal that read it earlier loses. Tests: `a_creation_removed_mid_preparation_is_superseded`, `a_reservation_taken_over_mid_preparation_is_superseded`, `a_failed_preparation_undoes_its_reservation`, `a_store_failure_mid_mutation_is_reported_and_undone`, `a_hint_is_kept_while_the_alias_names_its_index_again`, `a_hinted_addition_is_swapped_again_to_fence_a_removal`.
- AC-4: A deleted index leaves every alias that names it except one it is the last index of. A live mutation protecting it is aborted first. Detaching costs an exact count, and an alias that no longer names it costs one read. One that keeps changing is counted after four attempts. `aliases_of` confirms exactly the hinted aliases that name the index, one read each. Tests: `a_deleted_index_leaves_its_aliases_but_the_last_keeps_naming_it`, `an_alias_that_keeps_changing_is_counted_as_left`, `only_aliases_that_name_the_index_are_confirmed`.

## Test plan

`crates/meta/tests/descriptors/alias_mutations.rs` over the three backends, with exact meter counts. Races, in `alias_races.rs`, are planted through a test store that runs another writer's step just before a chosen request. Format helpers are tested in `crates/format/tests/alias_records.rs`.

## S3 request budget

For an alias of `n` members, a change that adds `a` and removes `r`:

| Operation | PUT-class | GET-class | LIST | DELETE | Depth |
|---|---|---|---|---|---|
| `create_alias` | 2 + n | 1 + 2n | 0 | 0 | 3 + 3n |
| `update_alias` | 2 + a + r | 2a + 2r | 0 | 0 | 2 + 3a + 3r |
| `update_alias`, stale | 0 | 1 ranged | 0 | 0 | 1 |
| `delete_alias` | 2 + n | 2n | 0 | 0 | 2 + 3n |
| `delete_alias`, stale | 1 | 1 ranged | 0 | 0 | 2 |
| recovering an update that added `a` | 1 + a | 2 + 2a | 0 | 0 | 3 + 3a |
| recovering a creation of `n` | 2 + n | 2 + 2n | 0 | 0 | 4 + 3n |
| `detach_from_aliases` | 1 an alias it drops from | 1 a hinted alias | 0 | 0 | the same |
| `aliases_of` | 0 | hint length | 0 | 0 | hint length |

The counts are for tombstone deletes; with conditional deletes, a delete's marker and tombstone become one DELETE. On fs, each swap also takes a ranged read. With change stripes configured, a delete also bumps its stripe, as an index's does. Aliases change at the rate of operator actions, and the cost scales with the members a change adds or removes, not with the aliases or indexes that exist. The Java's update also reads every kept member.

## Switch

None needed. Nothing calls these yet; they are the Java's protocol on the Java's bytes.

## Risks

- The added members are prepared one after another, as in the Java: a change adding many members is that many sequential round trips. Aliases are operator actions, and a parallel preparation can come with a measured need.
- Not preparing kept members relies on every writer following this protocol, which the Java's does. A kept member whose hint was lost some other way is not repaired; `aliases_of` and deletes still see the alias record itself.
