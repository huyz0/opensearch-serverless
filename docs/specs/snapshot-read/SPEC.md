# Snapshots, read-only

## Serves

[ADR 0038](../../adr/0038-snapshots-read-first.md): the shell's repositories and snapshots are readable over REST from a Rust node, byte-compatible, behind `serverless.snapshots.enabled` (default false).

## Acceptance criteria

- AC-1: `RepositoryRecord::parse` reads the shell's repository record (`name`, `type` defaulting to `fs`, `created_at` as a number or a numeric string, `settings` of scalar values) and refuses one with no name or `created_at`; a repository record the Java wrote (fixture) parses.
- AC-2: `SnapshotSummary::parse` reads the shell's snapshot record (identity, `storage_generation`, `shallow`, times, per-index uuid and shard count, captured shards counted) with the Java's refusals; a record the Java wrote (fixture) parses; the files are never copied.
- AC-3: `SnapshotState::parse` reads the state register value (marker, record, finished flag; the length is the Java's character count, so a non-ASCII marker is refused) and refuses cut, padded and oversized values; the released value reads as released; a register value the Java wrote (fixture) parses.
- AC-4: with the switch on, `GET /_snapshot/{repo}` answers 200 with the repository's name, `type` and `settings` in OpenSearch's shape, and 404 `repository_missing_exception` for none; an unreadable record answers 500.
- AC-5: `GET /_snapshot/{repo}/{name}` answers 200 `{"snapshots":[{...}]}` in the Java's shape for a snapshot a Java node took: `state` from the record (`IN_PROGRESS` for an index with shards and none captured, else `SUCCESS`, not from the register's flag), indices, times, `shards` totals (captured shards, failed 0); 404 `snapshot_missing_exception` for none.
- AC-6: lookup order: a released register is no snapshot even beside a surviving legacy blob; an absent register falls back to the legacy blob, whose key must equal `<repo>#<name>`; a held register's record key must equal it too; a mismatch answers 500. Nothing is cached between requests.
- AC-7: names: `#`, `,` and names `validateId` refuses answer 400 `invalid_snapshot_name_exception` with no request; `_all`, `_current`, patterns and comma lists answer 501, for repositories and snapshots; a key past 255 bytes answers as missing with no LIST.
- AC-8: a snapshot record or register value larger than `MAX_SNAPSHOT_RECORD_BYTES` answers 500 `snapshot_unreadable` and holds no more than the reserved buffer; a repository record past 1 MiB is unreadable.
- AC-9: `GET /_snapshot` lists every readable repository (an unreadable record is omitted), pages its LIST at 1,000 keys, is the only route that makes a LIST, and refuses over 256 repositories.
- AC-10: with the switch off the routes are not registered and no request is made; the write routes answer 501 with the switch on.
- AC-11: request budgets: `snapshot.repo.get.requests` 1, `snapshot.get.requests` 1 (register), `snapshot.get_legacy.requests` 2, `snapshot.get_missing.requests` 2, `snapshot.repos.list.requests` 1 LIST per 1,000 keys plus 1 GET a repository.

## Test plan

Fixtures: Java-written records under `fixtures/snapshots/` (generated with the Java classes as the bucket fixtures were). Unit tests in `crates/format`, store tests in `crates/meta` over the emulator, route tests in `crates/node/tests/server`, budgets in `docs/request-budgets.md`.
