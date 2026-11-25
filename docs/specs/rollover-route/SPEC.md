# Rollover: a new index behind an alias or a data stream

## Serves

R14.4 (data streams and aliases over REST). The coordinator decides a rollover ([coordinator-rollover](../coordinator-rollover/SPEC.md)) and the alias protocol swaps a record ([alias-mutations](../alias-mutations/SPEC.md)); no route joins them, so a data stream stays at generation 1. The Java shell's `RolloverHandler` (`serverless/shell/.../indexadmin/RolloverHandler.java`) is the port. Invariants served: every store call is metered; no LIST; fencing by compare-and-swap of the alias record; bounded memory.

## Change

- `oss_node::rollover_route::RolloverService` and the routes `POST /{index}/_rollover` and `POST /{index}/_rollover/{new_index}`; the query takes `dry_run` and the hints `timeout`, `master_timeout`, `cluster_manager_timeout`, `wait_for_active_shards`.
- The target is a literal alias or data stream name. The body, the refusals on what the target is, the conditions and the next name are `oss_coordinator::indices::rollover`'s.
- **A data stream** rolls to `.ds-<name>-<generation + 1>`, created under the template that wins the stream's name with the stream's timestamp field mapped as a date; a name already taken moves on a generation, up to 8 times, as core does. The record gets the new backing index at the end and the new generation.
- **An alias** with a write index gets the new index as a member and as its write index, the old members staying; an alias of one index and no write index moves to the new index. The new name is `new_index` or the old write index's trailing number plus one; a name with none is 400 "name the new index in the path". The new index is created as `PUT /{index}` creates one with no body, under the template that wins its name.
- The index is created first, then the record is swapped once. A swap lost to a concurrent change deletes the index just made and answers 409 `version_conflict_engine_exception`, saying whether it was removed.
- Answers `{"acknowledged","shards_acknowledged","old_index","new_index","rolled_over","dry_run","conditions"}`; a rollover that does not roll (a dry run, or no condition met) answers false for the first three and `rolled_over`.
- **Refused:** `max_docs` 501 `unsupported_condition` (a count needs a search over the index; `max_age` is evaluated), size conditions 501, `settings` and `mappings` in the body of an alias rollover 501 `unsupported_setting` (a stream refuses them 400, as core does), `aliases` in the body 501, a wildcard, `_all` or comma list as target 501.
- The routes need `all_access`; no new setting.

## Acceptance criteria

- AC-1: a stream rolls to the next generation, reads back with both backing indices and the new generation, and its older backing index deletes while the write index does not.
- AC-2: `max_age` decides: unmet answers `rolled_over:false` with the condition's key false and creates nothing; `dry_run` creates nothing; met rolls.
- AC-3: the new backing index has the template's mapping and the timestamp field as a date; a taken backing name is moved past by a generation.
- AC-4: an alias with a write index gains the new index as its write index; an alias of one index with none moves to the new index; a name with no trailing number is refused unless named in the path.
- AC-5: an index, a missing name, a pattern, a stream given a name or a shape, `max_docs`, a size condition, an unknown condition and aliases in the body are refused with the stated status and create nothing.
- AC-7: a swap lost to another change deletes the index just made, answers 409 saying so, and leaves the alias as the other change made it.
- AC-6: the requests of a one-index stream's rollover and of an alias's are the `rollover.*` rows of [request-budgets](../../request-budgets.md), none listing.

## Tests

`crates/node/tests/server/rollover_route.rs`.

## S3 request budget

A rollover costs the alias record read, the old index's descriptor read, the template markers and documents, the new index's create as `PUT /{index}`, and one alias update (the reservation, the new member's hint read and swap, the publish swap and the old hint's removal where it leaves). It scales with the members the swap adds or removes, never with the aliases, streams or indices that exist. A swap lost to a concurrent change adds the new index's delete. No LIST.

## Switch

None: a new route that changes no existing answer.

## Risks

- `max_docs` is refused, so a rollover by size of the index in documents is not available until a count can be asked of the search path from here; `max_age` and an unconditional rollover are.
- A crash between the new index's create and the record's swap leaves an index no name holds; the next rollover of a stream moves past its name, and an alias rollover names it taken (400) until it is deleted by name. The Java has the same window.
- A store failure after the swap published (while the old member's hint is removed) keeps the new index when the record names it, and answers the failure; that path is not covered by a test.
- A dry run names the next backing index without checking that it is free; a real rollover moves past a taken name.
