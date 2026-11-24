# Repository registration

## Serves

[ADR 0040](../../adr/0040-repository-registration.md): a Rust node registers, re-registers, verifies and removes a snapshot repository over REST, writing the record the Java shell writes, behind `serverless.snapshots.enabled` (default false).

## Acceptance criteria

- AC-1: `RepositoryRecord::to_bytes` writes the shell's record (`name`, `type`, `created_at`, `settings`), and a record written here reads back equal, and equal in content to the Java-written `fixtures/snapshots/repository.json` once both are parsed; a Java test reads a Rust-written record in the mixed-fleet run (settings key order is not compared).
- AC-2: `PUT` and `POST /_snapshot/{repo}` store the record in one PUT, an upsert, with `created_at` from the node's clock; the answer is `{"acknowledged":true}` 200; a second registration replaces the first; `verify` is accepted as a URL parameter and changes nothing.
- AC-3: order: a name starting `_` (not `_all`) answers 501 `not_implemented` before the body is read; `_all` and an invalid name answer 400 in `validateId`'s words, with no request; then `type` other than `fs` answers 501 `unsupported_repository_type` and `crypto_settings` 501 `unsupported_repository_setting` (type first); a missing `type` is `fs`; settings are stored as text (`5` as `"5"`, `true` as `"true"`), nested objects as dotted keys, an array or null setting answers 400; a body past 1 MiB is refused and a failed reservation answers 429.
- AC-4: `DELETE /_snapshot/{repo}[,{repo}]` validates every name first (`_all`, `_x`, patterns and invalid names answer 400 `invalid_snapshot_name_exception`), answers 404 `repository_missing_exception` for an unregistered name, 409 `repository_in_use` while a readable held snapshot names it, and deletes the record otherwise; `{"acknowledged":true}`.
- AC-5: the in-use scan lists `snapshots/<repo>#` and the v2 prefix, reads each key found until one is held, omits released, mismatched and unreadable ones as the Java does, stops at the first held snapshot, and answers 500 `too_many_snapshots_to_check` past 1,000 keys in total.
- AC-6: `POST /_snapshot/{repo}/_verify` answers 200 with `{"nodes":{id:{name}}}` for a registered repository, 404 otherwise, 500 `repository_unreadable` for a record that does not parse, and `_x/_verify` answers 400.
- AC-7: with the switch off none of these routes is registered and no request is made.
- AC-8: request budgets: `repository.put.requests` 1; `repository.delete_empty.requests` 4 (a GET, two LISTs, a DELETE); `repository.verify.requests` 1 (the Java spends 2).

## Out of scope

Taking, restoring and deleting snapshots; repository types other than `fs`.
