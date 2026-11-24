# Snapshots, read-only

## Serves

[ADR 0038](../../adr/0038-snapshots-read-first.md): the shell's repositories and snapshots are readable over REST from a Rust node, byte-compatible, behind `serverless.snapshots.enabled` (default false).

## Acceptance criteria

- AC-1: `RepositoryRecord::parse` reads the shell's repository record (`name`, `type` defaulting to `fs`, `created_at`, `settings`) and refuses one with no name or no `created_at`; a recorded Java record parses.
- AC-2: `SnapshotRecord::parse` reads the shell's snapshot record (identity, `storage_generation`, `shallow`, times, per-index uuid, shard count, mapping, settings and per-shard term, files and lengths) with the Java's refusals (a bad `shallow`, a negative term, a file or length that is not a string or number); a recorded Java record parses.
- AC-3: `SnapshotState::parse` reads the state register value (marker, record, finished flag) and refuses trailing bytes and a record whose key is not the one asked for; the released value reads as no snapshot.
- AC-4: with the switch on, `GET /_snapshot/{repo}` answers 200 with the repository's name, `type` and `settings` in OpenSearch's shape, and 404 `repository_missing_exception` for none.
- AC-5: `GET /_snapshot/{repo}/{name}` answers 200 with the snapshot's name, uuid, state (`IN_PROGRESS` for an unfinished one, else `SUCCESS`), indices, start and end times, shard totals, for a snapshot a Java node took, and 404 `snapshot_missing_exception` for none or a released one; `_all` and wildcards answer 501.
- AC-6: `GET /_snapshot` lists every readable repository (an unreadable record is omitted), and is the only route that makes a LIST.
- AC-7: with the switch off the routes are not registered and no request is made; the write routes answer 501 with the switch on.
- AC-8: request budgets: `snapshot.repo.get.requests` 1, `snapshot.get.requests` 1 (2 for a legacy record), `snapshot.repos.list.requests` 1 LIST page plus 1 GET a repository.

## Test plan

Fixtures: Java-written records under `fixtures/snapshots/` (generated with the Java classes as the bucket fixtures were). Unit tests in `crates/format`, store tests in `crates/meta` over the emulator, route tests in `crates/node/tests/server`, budgets in `docs/request-budgets.md`.
