# Snapshot delete, shallow

## Serves

[ADR 0042](../../adr/0042-snapshots-taken-shallow-first.md): a Rust node deletes a shallow snapshot as the Java shell does (`SnapshotRecords.deleteSnapshot`), served when `serverless.snapshots.enabled` and `serverless.snapshots.take.enabled` are on, so the stuck-take case has a way out.

## Change

`DELETE /_snapshot/{repo}/{name}` reads the name's register; a held snapshot (finished or still being taken) has its register released by a compare-and-swap (up to 16 tries), then its discovery marker deleted. Answers `{"acknowledged":true}`. The janitor stops pinning the snapshot's indices once the register is released; a marker whose register is released is skipped by every reader, so a failure between the two steps leaves nothing that names storage.

Refused: no snapshot of the name 404 `snapshot_missing_exception`; a selector (`_all`, `_current`, `*`, a comma) 501; an invalid name 400; a snapshot that copied its data (a standard snapshot, which needs its data prefix deleted, a LIST) one only a legacy record holds, and any name in a stock repository ([stock export](../stock-snapshot-export/SPEC.md)) 501 `unsupported_operation`, left untouched; either switch off 501 as before.

## Acceptance criteria

- AC-1: A delete releases the name, stops the janitor pinning its indices, and removes the marker (the repository can then be removed); the name can be taken again. Test: `a_delete_releases_the_name_and_the_pin_and_the_name_can_be_taken_again`.
- AC-2: A missing snapshot, and a second delete, are 404. Test: `deleting_what_is_not_there_is_404_and_a_second_delete_too`.
- AC-3: A selector is 501, an invalid name 400, and with the switch off 501. Test: `a_selector_or_a_bad_name_is_refused_and_a_switch_off_is_501`.
- AC-4: A snapshot that copied its data is refused 501 and left in place. Test: `a_snapshot_that_copied_its_data_is_refused_and_left_alone`.
- AC-5: A take still in progress can be deleted, and the take then fails to finish and cleans up after itself. Test: `a_take_still_in_progress_can_be_deleted`.
- AC-6: One delete costs 3 requests (budget row `snapshot.delete.requests`): the register read, the swap, the marker DELETE; no LIST. Test: `a_delete_costs_what_the_spec_budgets`.

## S3 request budget

3 requests, `snapshot.delete.requests` in [request-budgets](../../request-budgets.md), exact, whatever the snapshot's size. A missing name costs one register read (and a legacy HEAD), and one more read of the repository record to tell a stock repository, which is refused 501 instead of 404.

## Switch

The take's two switches; no new setting. Taking and deleting are served together.

## Risks

A shallow snapshot's delete reclaims no data, as none was copied. A Java node reads a Rust-released register as released; the mixed-fleet delete proof is AC-5 of [snapshots-default-on](../snapshots-default-on/SPEC.md).
