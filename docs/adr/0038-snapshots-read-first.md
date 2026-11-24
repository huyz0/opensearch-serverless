# 0038: Snapshots arrive read-only first, over the shell's records, behind a switch

**Status:** Proposed

## Context

The shell keeps repositories and snapshots in the deployment's own object store ([R14](../roadmap.md)): a repository is a record `repositories/<name>`; a snapshot is a record whose current state sits in a CAS register `snapshot-states/<sha256 of "<repo>#<name>">` (a name's lifetime is a unique storage generation; the register holds the record, a discovery marker and a finished flag), with a discovery marker blob under `snapshots/` that a list walks. A standard snapshot copies shard blobs under `snapshot-data-v2/<generation>/<index>/<shard>`, a shallow one references the live shard's blobs. A Rust node shares the bucket with Java nodes, so whatever it writes the Java must read, and whatever the Java wrote it must read. The janitor already reads snapshot records for their pins ([janitor-snapshot-pins](../specs/janitor-snapshot-pins/SPEC.md)); nothing else does.

Taking, restoring and deleting a snapshot touch the write path, shard blobs, pins and index creation. Reading does not. Doing the read side first gets the format right against the Java's own records before anything can write one.

## Decision

- **Slice one is read-only:** `GET /_snapshot`, `GET /_snapshot/{repo}`, `GET /_snapshot/{repo}/{snapshot}` (one exact name, `_all` and `*` refused for now) answer from the shell's records. `PUT`, `DELETE`, `_restore` and the snapshot create route answer `501 unsupported_operation` with a reason naming the slice. Nothing here writes a blob, so a Rust node cannot corrupt a Java snapshot; the legacy-record adoption the Java does on a read (creating the state register) is not done: a legacy record is read as it is.
- **Switch:** `serverless.snapshots.enabled`, default false, a node setting. Off, the routes are not registered (the pre-change 400 answer for an unknown route) and no request is made. It turns on by default only in a PR of its own when a mixed-fleet read of a Java snapshot has been measured, per the switch rule; it has no downside while read-only (no request unless asked) but the answers need the Java records of a live deployment to be verified first.
- **Formats** are the Java's, read strictly and written nowhere: `RepositoryDescriptor` (`name`, `type`, `created_at`, `settings`), `SnapshotRecord` (the fields `SnapshotRecord.fromStream` takes, including `storage_generation`, `shallow`, per-index `uuid`, `number_of_shards`, `mapping`, `settings` and per-shard `term`, `files`, `lengths`), and the state register's value (a vInt-prefixed marker string, a vInt-prefixed record, a boolean; the released form `{"_released":true}` reads as no snapshot). Fixtures are recorded Java outputs under `fixtures/snapshots/`, as for the bucket manifests.
- **Cost, no LIST on the common path:** `GET _snapshot/{repo}` is 1 GET; `GET _snapshot/{repo}/{name}` is 1 GET (the state register) plus, for a legacy snapshot with no register, 1 GET of `snapshots/<repo>#<name>`. `GET _snapshot` lists the repositories container: one LIST page and 1 GET a repository, the same justified listing as the Java's (a repository is operator-registered and human-scale, a handful; a request the caller makes, never a timer). A repository record larger than the pooled buffer reads as unreadable and is omitted from a listing as the Java omits it. Listing the snapshots of a repository (`_all`) is a LIST of the discovery markers and a GET each, so it is **not** in this slice; a later ADR decides it with its budget row.
- **Placement:** record parsing in `oss-format` (`repository_record.rs`, `snapshot_record.rs`; `snapshot_pins.rs` stays the pins' reader and reuses the new parser later), the store reads in `oss-meta`, the routes in `oss-node` as the other admin routes are. No new crate in this slice; the take and restore slices decide `oss-snapshot`.

## Consequences

Easier: the formats are proved against the Java's records before anything writes them; the coordinator's snapshot admin surface starts answering for deployments that Java nodes snapshot. Harder: `_all` and repository registration are visibly missing, and clients that PUT a repository first get a 501 until the write slice. Ruled out: a Rust node adopting legacy records (a write) on read.

## Open for the write slices

Native repository type only (`fs`, the shell's `TYPE_NATIVE`); others refused with the Java's `unsupported_repository_type`. A restore must register snapshot pins with the janitor and create the index under a new uuid.
