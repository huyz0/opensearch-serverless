# Repository registration

## Serves

[ADR 0039](../../adr/0039-repository-registration.md): a Rust node registers, re-registers, verifies and removes a snapshot repository over REST, writing the record the Java shell writes, behind `serverless.snapshots.enabled` (default false).

## Acceptance criteria

- AC-1: `RepositoryRecord::to_bytes` writes the shell's record (`name`, `type`, `created_at`, `settings` in order), and the Java reads what it writes and the Rust reads what the Java wrote (`fixtures/snapshots/repository.json` round trips).
- AC-2: `PUT` and `POST /_snapshot/{repo}` store the record in one PUT, upsert, with `created_at` from the node's clock; the answer is `{"acknowledged":true}` 200; a second registration replaces the first.
- AC-3: the body: `type` other than `fs` answers 501 `unsupported_repository_type`, `crypto_settings` 501 `unsupported_repository_setting`; nested settings flatten to dotted keys; an invalid name answers 400 and a name starting `_` 501, each before any request; a body past 1 MiB is refused and a failed pool reservation answers 429.
- AC-4: `DELETE /_snapshot/{repo}` refuses `_all`, patterns and lists with 400, answers 404 for an unregistered repository, 409 `repository_in_use` while a readable held snapshot names it, and deletes the record otherwise.
- AC-5: the in-use scan lists `snapshots/<repo>#` and the v2 marker prefix, reads each key found until one is held, ignores released and unreadable ones as the Java does, and refuses past 1,000 keys.
- AC-6: `POST /_snapshot/{repo}/_verify` answers 200 for a registered repository and 404 otherwise.
- AC-7: with the switch off none of these routes is registered and no request is made.
- AC-8: request budgets: `repository.put.requests` 1; `repository.delete_empty.requests` 4 (a GET, two LISTs, a DELETE); `repository.verify.requests` 1.

## Out of scope

Taking, restoring and deleting snapshots; repository types other than `fs`.
