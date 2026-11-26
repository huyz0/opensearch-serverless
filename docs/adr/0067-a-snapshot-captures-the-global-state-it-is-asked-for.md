# 0067: A snapshot captures the global state it is asked for, and a restore may bring it back

**Status:** Proposed

## Context

Stock OpenSearch's global state is the cluster-wide metadata: persistent settings, index and component templates, ingest and search pipelines and stored scripts. A take with `include_global_state=true` keeps it in the snapshot and a restore with `include_global_state=true` puts it back. A snapshot here records only indices; both nodes accept `include_global_state: false` on a take and a restore and refuse `true` on a restore with 501 (`restore-default-flags`).

The project owner has decided to lift the refusal in both implementations. The global state here is five families of records, each with its own store and bounds, which this ADR names rather than treating as one thing:

- Persistent cluster settings: one register (`ClusterConfig`, at most 256 KiB).
- Index templates, component templates, ingest pipelines, search pipelines and stored scripts: five **document sets** (`DocSet`), each a marker register naming immutable content blobs, at most 1,000 documents of at most 1 MiB. A replaced or deleted document's content blob is deleted at once (`drop_content`), so a snapshot cannot reference a content blob and expect it to stay.

Security configuration (roles, role mappings, internal users, tenants) is not global state in stock either and stays out. Legacy templates do not exist here.

## Decision

### The format

The snapshot record may carry one top-level member, written only when the take asked for it:

```json
"global_state": {
  "persistent_settings": { "<key>": "<value>" },
  "index_templates":     { "<name>": { ...the document... } },
  "component_templates": { ... },
  "ingest_pipelines":    { ... },
  "search_pipelines":    { ... },
  "stored_scripts":      { ... }
}
```

Each member is present only when its set is not empty; a document is the JSON the REST body held, stored as a value, not a string. The state is **embedded in the record**, not a blob beside it: a shallow snapshot is one register today, so a delete stays one swap and a marker delete, no old node leaks a blob it does not know, and the record's existing 16 MiB bound applies. The state itself is bounded to 4 MiB; a take whose state is larger is refused 400 `snapshot_too_large` before it writes anything, naming the set that pushed it over.

The change is additive. A record without it is a snapshot with no global state. A reader that does not know it must skip a top-level member: the Rust readers skip unknown top-level fields; the Java `fromStream` reads the record as a map and ignores what it does not name (to be verified by a test against the real reader before the writer ships, and a Java node that does not skip it refuses the whole record, which would make the gate below load-bearing). The summary parsers never read it, so a corrupt state cannot break listing, delete or the janitor's pins; only a restore that asks for it reads it, strictly, and a malformed state is 500 `snapshot_unreadable`.

### The take

A setting, `serverless.snapshots.take.capture_global_state` (`OSS_SNAPSHOTS_TAKE_CAPTURE_GLOBAL_STATE`, the same key in Java, with a how-to entry), default off. Off, the take is byte for byte what it was and `include_global_state: true` on a take still means nothing, as today. On, a take with `include_global_state: true` reads the cluster settings register and, for each of the five sets, the marker and then each document, and writes them into the final record only (the provisional record carries none). A take that does not ask for it reads none of it. Stock's default for a take is `true`; here it stays `false` (a **Differs**, as `include_aliases` is), because the cost below is for every take otherwise.

Cost, listed as budget rows: one GET for the settings, and for each set one GET for the marker and one GET for each document, so at most 1 + 5 x (1 + 1,000) requests, bounded, nothing per index or per elapsed time, no LIST, no PUT. An empty set costs its marker GET (an absent marker is one GET that finds nothing). The reads of one set are independent and run with the same bounded fan-out the take uses elsewhere, not unbounded.

The point in time is per register and per set, not atomic across them, and not atomic with the shards: a document changed during the take is in the record as the read saw it. That is documented, not fixed.

### The restore

`include_global_state` (a boolean, default false) is read on the restore; every other refused key stays refused. With `true`, after every restored index is whole (and, with ADR 0065, after the aliases), the restore applies what the record holds: each document is put into its set under its name, replacing a document of that name (stock replaces), and the persistent settings are merged into the cluster settings register key by key, the snapshot's value winning. Nothing is deleted: a document the snapshot did not hold stays. A snapshot with no global state restores nothing and answers `"global_state_restored":0`.

Cost: for each document one `DocSet.put` (the content blob, then one marker swap, and a delete of a replaced blob), and one read-and-swap of the cluster settings; every restored document is a new write, which a restore asked for explicitly. It is bounded by the 4 MiB state. The restore checks before any write that the state fits each set (at most 1,000 documents after the merge, 1 MiB a document) and that the cluster settings merge stays inside their bound, refusing 400 and writing nothing otherwise; it checks a document name against the set's rules in the same pass.

Failure: a document or the settings that cannot be applied never removes an index. A synchronous answer is 200 with `"global_state_restored":N` and `"failed_global_state":["index_template/<name>", ...]` inside the `snapshot` object, as `failed_aliases` is; a background restore (ADR 0063) logs one line and adds one to its failed count. A retry is refused (the indices exist), so the missing documents are put by hand. This is a **Differs**.

### Both implementations

The Rust node writes and reads the member and builds the setting, the take and the restore. The Java shell changes `SnapshotRecord` (carried through `fromStream`, the constructors and `toBytes`, including the legacy adoption path), the take, the restore and `SnapshotRequestBodies` (`include_global_state` leaves `RESTORE_REFUSED_KEYS`), and reads its own `TemplateStore` and cluster config the same way. Fixtures under `fixtures/snapshots/` gain a record with global state written by each side and read by the other, and corrupt cases; the Java reader's tolerance of the unknown member is a test before the Rust writer is enabled. Docs and tables change with it: the key table and restore-default-flags AC-2 and AC-4, `docs/compatibility.md`, the take and restore specs, the budget table, the OpenAPI document and the crate READMEs.

## Out of scope

- Security configuration, which stock does not hold in global state either.
- A stock repository: a restore from one does not read its global metadata here, and the export to one does not write it.
- Deleting what the snapshot lacks, and restoring a single set (`feature_states` and the like).
- `partial` (ADR 0066).

## Alternatives rejected

- **A content-addressed reference to each document's blob:** a replaced or deleted document's blob is deleted at once, so the reference would dangle; keeping those blobs would need a pin the document sets do not have.
- **A separate global-state blob beside the register:** a delete and the janitor must know it, and a node that does not know it leaks it; embedding keeps one register.
- **Capturing the state on every take, as stock's default does:** every take would read up to 5,000 documents for a feature most deployments do not use.
- **Replacing a set wholesale on restore:** deletes documents the operator added since; restoring by name never deletes.
- **One setting for global state and aliases:** the two have different costs and different readers.

## Consequences

A snapshot can carry the operator's configuration and a restore can bring it back to a new deployment. The cost is only paid by a take that asks for it, with the setting on, and by a restore that asks for it. Until the Java reader skips the member, the setting stays off; flipping it is an operator decision once every node reading the repository has both changes, and the node does not enforce it.
