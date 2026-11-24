# 0039: Repositories are registered and removed over REST, the Java's way, before snapshots are taken

**Status:** Proposed, 2026-11-24 (awaiting independent review)

## Context

[ADR 0038](0038-snapshots-read-first.md) made repositories and snapshots readable. The next piece with no shard data in it is the repository itself: `PUT`, `POST` and `DELETE /_snapshot/{repo}` in the shell's `RepositoryHandler` and `RepositoryStore`. A repository is one blob, `repositories/<name>`, which a Java node and a Rust node both read. Taking, restoring and deleting snapshots write shard blobs and pins and are separate decisions.

## Decision

- **Switch:** the routes are served under `serverless.snapshots.enabled` (ADR 0038); a second switch is not added, because registering a repository writes one small blob and changes nothing else. With the switch off the routes are not registered, as before.
- **`PUT`/`POST /_snapshot/{repo}` is an upsert**, as the Java's: one PUT of `repositories/<name>`, the record `{"name","type","created_at","settings"}` with `created_at` the node's clock in milliseconds, rewritten on each registration. No read first, no condition: two callers registering one name is the operator's coordination, as in the Java. 1 PUT.
- **Request.** The body is an object with optional `type` (only `fs`; any other answers 501 `unsupported_repository_type` in the Java's words), `settings` (flattened to dotted keys with scalar values read as the Java's `Settings` flat form: a nested object becomes `a.b`, a number or boolean is kept as text in the record only where the Java wrote text; the value forms the read slice already accepts), `verify` (accepted, a no-op: there is one store) and `crypto_settings` (501 `unsupported_repository_setting`). A name goes through `Names.validateId` (400 `invalid_snapshot_name_exception` is the read slice's kind; here the kind is the Java's, `illegal_argument_exception`). A name starting with `_` answers 501, as the Java's. The body is bounded at 1 MiB (the record's read limit), through the request-body pool.
- **`DELETE /_snapshot/{repo}`** refuses `_all`, patterns and comma lists (400 `illegal_argument_exception`, the Java's words), answers 404 `repository_missing_exception` for an unregistered name, 409 `repository_in_use` while any readable, held snapshot names the repository, and otherwise deletes the blob. Cost: 1 GET (exists), then the Java's snapshot listing, then 1 DELETE. The listing is two LISTs (`snapshots/<repo>#` and the v2 marker prefix) and one GET per key found to see whether it is held; with no snapshots that is 2 LISTs and a repository delete costs 1 GET, 2 LISTs and 1 DELETE. A LIST is unavoidable here: the Java keeps no per-repository count, and an index document would be a Rust-only write the Java never updates. It is an operator action, never on a timer, and carries `// LIST:`. The scan stops at the first held snapshot (409) and refuses past 1,000 keys with 500 `too_many_snapshots_to_check` rather than fan out without bound.
- **`POST /_snapshot/{repo}/_verify`** answers the Java's body (this node as the only node) after confirming the repository exists: 1 GET.
- **Idempotent, visible:** a repeated `PUT` of the same body answers 200 and rewrites the blob; a `DELETE` of a name already gone answers 404.
- **Answers:** `{"acknowledged":true}` with 200.
- **Placement:** the record writer in `oss-format` (`RepositoryRecord::to_bytes`, tested against the record the Java wrote), the store writes and the in-use check in `oss-meta`, the request decisions in `oss-coordinator`'s `snapshot_admin`, the routes in `oss-node`'s `snapshot_route`, replacing the 501 handlers for these paths only.

## Consequences

Easier: a deployment can register its repository from a Rust node, and the repository's registration is proved byte-compatible with the Java in both directions before snapshots write anything. Harder: a delete's in-use check costs LISTs, the price of the Java's format. Snapshot creation, restore and snapshot deletion stay 501.

## Alternatives rejected

- A repository-count index document: a Rust-only write, stale as soon as a Java node writes a snapshot.
- Refusing re-registration: the Java upserts, and clients written against OpenSearch send repeat `PUT`s.
- Deleting without the in-use check: leaves snapshots naming a repository that cannot be read.
- A second switch: no downside to separate.
