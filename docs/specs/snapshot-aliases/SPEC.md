# Snapshot aliases

## Serves

Parity plan R14, [ADR 0065](../../adr/0065-a-snapshot-captures-the-aliases-of-its-indices.md): a take records the aliases of the indices it captures, and `POST /_snapshot/{repo}/{name}/_restore` with `include_aliases: true` adds them to the indices it creates. The bytes are shared with the Java shell, which makes the same change. Builds on [snapshot-take](../snapshot-take/SPEC.md), [snapshot-restore](../snapshot-restore/SPEC.md), [restore-background](../restore-background/SPEC.md) and [alias-mutations](../alias-mutations/SPEC.md).

## Acceptance criteria

- AC-1: An index entry of a snapshot record may carry `aliases` (`{"<name>":{}}`, or `{"is_write_index":true}` for an explicit write member; written only when not empty; 1 to 255 bytes a name, at most 64) and `aliases_truncated`. A record written here reads back as drafted, the bytes the Java shell writes read here and are written back byte for byte, and a record without the fields reads as none. The restore's reader refuses a malformed field (not an object, a flag that is not a boolean, an empty or over-long name, more than 64, a repeated name) as a corrupt record; a summary parse (list, get, delete, the janitor's pins) never reads the fields, so a corrupt one cannot break them. Test: round trip, bounds at the edge, each corrupt case, the summary parse over a malformed field, and the Java-written fixture.
- AC-2: With `serverless.snapshots.take.capture_aliases` on, a take reads each distinct alias named by the captured indices' `aliased_by` hints once, however many indices name it, and records an alias for an index only when the register still lists the index; a data stream is not an alias; a creation not yet published reads as absent; an index whose hint names more than 64 records the first 64 names (sorted) and `aliases_truncated`. No LIST. Test: one read each across two indices in one alias, an unconfirmed hint, a data stream, 65 hint names (64 reads).
- AC-3: The setting is off by default. Off, the take reads and writes what it did before: the existing take tests and `snapshot.take.requests` pass unchanged. Test: the setting's default and its parse, and the unchanged rows.
- AC-4: `include_aliases` (a boolean; any other value is 400), `rename_alias_pattern` and `rename_alias_replacement` are accepted; `alias_write_index_policy` and the other refused keys stay 501. Either rename key alone is ignored, and so are both without `include_aliases: true`. Absent, `include_aliases` is false. Test: the key table cases.
- AC-5: Before any write the restore plans the aliases and refuses with 400 `invalid_alias_name_exception`: a name (after the rename) that is empty, over 255 bytes, starts with `_`, is not a valid name, equals a restored index, or is held by an index or a data stream; and with 400 more than 6,400 distinct aliases. Nothing is created. Test: each refusal asserting no index and no write; the bound at the edge.
- AC-6: After every restored index is unmarked, one mutation for each distinct alias adds all its restored members, creating the alias when it is absent. A restored member is the write index only when the snapshot said so and the alias has no write target, explicit or implicit; the first such member in plan order wins; an existing write target keeps the role, an implicit one made explicit so a second member leaves the alias with a target. Test: new alias with two claimants, an existing alias with an implicit and with an explicit target, a multi-member alias with none, a repeat that writes nothing.
- AC-7: An alias that cannot be added never removes an index. A synchronous restore answers 200 and names it in `failed_aliases`; the rest still run. Test: an alias another writer holds with a far deadline.
- AC-8: A background restore adds the aliases after the last mark clears, on its own thread, inside the reservation. A failure there is one line on standard error and one in the failed count; a cancel names the aliases not yet added the same way and keeps the indices; the indices are never deleted by the alias step, so a panic in it does not run the cleanup. Test: the aliases land after the copy and the failed count stays 0.
- AC-8b: A restore that does not wait, with the background switch off, still adds its aliases before it answers `{"accepted":true}`; a failure has no body to carry it and is logged. Test: the aliases exist after the answer.
- AC-9: With `include_aliases: true` the answer's `snapshot` object carries `aliases_restored` (distinct aliases added, 0 when the snapshot recorded none), `failed_aliases` when any failed and `aliases_truncated` (the restored indices whose record was cut) when any; otherwise it is the restore's answer unchanged. Test: the render, and a restore of a snapshot taken with the setting off.
- AC-10: Costs are in the budget table: `snapshot.take_aliases.requests` (the take plus one read for each distinct alias) and `snapshot.restore_aliases.requests`. A restore without `include_aliases` costs what it did (`snapshot.restore_two.requests`). Test: both rows spent.

## Bounds

At most 64 aliases an index in a record; at most 6,400 distinct alias reads in a take and 6,400 distinct aliases in a restore; the alias step holds three register buffers (`pit_buffer_bytes` each), reserved with the restore's other buffers.

## Switch

`serverless.snapshots.take.capture_aliases` (`OSS_SNAPSHOTS_TAKE_CAPTURE_ALIASES`), default off until the Java shell reads the fields; the follow-up that flips it names the measured rows. `include_aliases` is the request's own switch.

## Risks

A snapshot taken with the setting on and restored by an older Java node loses its aliases silently and that node still refuses the keys. The alias reads of a take run one after another, not 16 at a time as the ADR allows. The restore's pre-write check of an alias name reads the register and the mutation reads it again; the ADR's reuse of the first read is not done. An alias read that fails fails the whole take (a record is not written with aliases missing), where a restore never fails for an alias. An alias changed during a take may or may not be in the record. A client that retries a restore after `failed_aliases` is refused (the indices exist), so the aliases are added by hand.

## Build order

1. The fields in `oss_format` (AC-1). 2. The take's confirm step and the setting (AC-2, AC-3). 3. The keys and the plan (AC-4, AC-5). 4. The mutation, then the route and the background thread (AC-6 to AC-9). 5. The budget rows and docs (AC-10).
