# Restore: `index_settings` and `ignore_index_settings`

## Serves

[ADR 0045](../../adr/0045-shallow-snapshot-restore.md) refused both keys with 501; core's restore accepts them. A restore of a native or a stock snapshot ([stock-snapshot-restore](../stock-snapshot-restore/SPEC.md)) now changes the settings of the indices it creates, as core's `RestoreService` does: drop the ignored ones, then apply the given ones.

## Acceptance criteria

- AC-1: `index_settings` is an object, nested or flat, with or without the `index.` prefix; each leaf replaces the captured value of that key in every index the restore creates, and a null leaf removes the captured setting. Values are checked as an index create checks them (a bad value is 400 `illegal_argument_exception` naming the setting), and a key this node does not keep is 501 `unsupported_index_option` naming it, as at create. Tests: `crates/coordinator/tests/it/restore_settings.rs` (the parse and the merge) and `crates/node/tests/server/restore_settings.rs` (over REST, native and stock repositories).
- AC-2: `ignore_index_settings` is a comma-separated string or an array of names, with or without the `index.` prefix, a trailing `*` matching any suffix (a `*` anywhere else is 400, not a silent no-match); each captured setting it names is absent from the created index. It is applied before `index_settings`, so a key in both ends up with the given value, as core does. Tests as AC-1.
- AC-3: `index.number_of_shards`, `index.version.created`, `index.uuid`, `index.creation_date` and `index.history.uuid` cannot be modified or removed: a restore naming one in either key is 400 `snapshot_restore_exception` (`cannot modify setting [..] on restore`, `cannot remove setting [..] on restore`), before any write; a wildcard that would match one removes everything else it matches and leaves that one, as core does. Tests as AC-1.
- AC-4: A request naming neither key restores exactly as before: the same descriptor bytes and the same requests (the native budget rows are unchanged), and every other refused key is still refused with its reason. Tests: the existing restore server tests and the request-budget rows `snapshot.restore_two.requests` and `snapshot.restore_cleanup.requests`.
- AC-5: The change costs no request: it is made on the settings already read, before the descriptor is written. Test: a restore with both keys makes the same number of requests as one without.

## S3 request budget

None added: the settings are changed in memory between the read of the snapshot record and the descriptor create.

## Risks

A setting the node reads at open (`index.routing_partition_size`, `index.routing_shards`) changes a restored index's routing: the create's checks apply to each value given, against the captured shard count; a removal is not re-checked against the other routing key. A Java shell node still refuses these keys (501), so a client cannot rely on them against a mixed fleet until the shell takes them too.
