# 0068: A snapshot captures the global state it is asked for, and a restore may bring it back

**Status:** Accepted

## Context

Stock OpenSearch's global state is the cluster-wide metadata: persistent settings, index and component templates, ingest and search pipelines and stored scripts. A take with `include_global_state=true` keeps it in the snapshot and a restore with `include_global_state=true` puts it back. A snapshot here records only indices; both nodes accept `include_global_state: false` on a take and a restore and refuse `true` on a restore with 501 (`restore-default-flags`).

The project owner has decided to lift the refusal in both implementations. The global state here is five families of records, each with its own store and bounds, which this ADR names rather than treating as one thing:

- Persistent cluster settings: one register (`ClusterConfig`, at most 256 KiB).
- Index templates, component templates, ingest pipelines, search pipelines and stored scripts: five **document sets** (`DocSet`), each a marker register naming immutable content blobs, at most 1,000 documents of at most 1 MiB. A replaced or deleted document's content blob is deleted at once (`drop_content`), so a snapshot cannot reference a content blob and expect it to stay.

Security configuration (roles, role mappings, internal users, tenants) is not global state in stock either and stays out. Legacy templates do not exist here.

## Decision

### The format

The snapshot record may carry one top-level member, written only when the take captured it:

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

Each member is present only when its set is not empty; a document is the JSON the REST body held, stored as a value. Settings are scalars: a take skips a list-valued setting (the cluster settings cannot re-apply one) and logs one line naming the key. The state is **embedded in the record**, not a blob beside it, so a delete stays one swap and no old node leaks a blob it does not know. Its size is the encoded size of the member; it is at most 4 MiB, and a take whose state is larger is refused 400 `snapshot_too_large` naming the set that pushed it over.

The change is additive for readers: the Rust readers skip an unknown top-level field and Java `fromStream` reads the record as a map and ignores what it does not name. **A rewrite is not safe.** Java `toBytes` and `withStorageGeneration` re-encode a record and the legacy adoption path goes `fromStream` then `encode`, so a Java node that does not carry the member deletes it with no error. The Java change therefore carries `global_state` as an opaque, unvalidated structure through `fromStream`, the constructors and `toBytes`, and a test pins that a `fromStream` then `toBytes` round trip keeps it byte for byte. The summary parsers never validate it; only a restore that asks for it reads it, strictly, and a malformed state is 500 `snapshot_unreadable`.

**Ordering rule (a precondition, not a hope).** The Java carry-through and its round-trip fixture land and ship first; only then may the Rust setting be turned on anywhere that shares a repository with a Java node. Until then the setting stays off. A predicate backs it: a tree check fails when a record member written by the Rust format crate has no fixture under `fixtures/snapshots/` that the Java reader round-trips, with a planted violation in `scripts/selftest-checks.py` and a row in `docs/mechanical-gates.md`.

### Security

Global state is configuration that `manage_snapshots` cannot write directly: stored scripts need `all_access`, pipelines and templates have their own permission groups. A take with `include_global_state: true` and a restore with it therefore need the action `cluster:admin/global_state/snapshot`, which only `all_access` grants. It is deliberately outside the `cluster:admin/snapshot/*` prefix, so the wildcard `manage_snapshots` holds cannot match it. The static config sees only action names, so the take and restore routes read the body flag and make a second authorisation check for this action after parsing and before any read or write. A caller without it gets 403 before any read or write. Tests: a `manage_snapshots` user is refused on both, and a snapshot with global state is still listable and restorable without it. A stock export drops global state (Out of scope), so it never leaves through that door.

### The take

A setting, `serverless.snapshots.take.capture_global_state` (`OSS_SNAPSHOTS_TAKE_CAPTURE_GLOBAL_STATE`, the same key in Java, with a how-to entry), default off. Off, a take is byte for byte what it was. A take that asks for `include_global_state: true` with the setting off is not a silent no-op: the answer's `snapshot` object, and the snapshot as GET returns it, carry `"include_global_state": false`, true only when the state was captured. Stock's default is `true`; here it stays `false` (a **Differs**), because the cost below would be on every take.

The state is read **first**, before any shard work, so a refusal for size is cheap and no shard is copied for nothing. Reads are **sequential** (one buffer; `DocSet::for_each` has no fan-out): the cluster settings, then for each of the five sets its marker and each document. A document deleted mid-walk restarts that set from its marker once; a second restart fails the take 409 `snapshot_state_changed`. Exact rows (the `d` documents of a set are counted once per pass): an empty state is 6 GETs; with `d_i` documents in set `i` it is `1 + 5 + sum(d_i)`; the bound is 1 + 5 x (1 + 1,000) = 5,006 per pass and 10,011 with one restart. Nothing scales with indices or time, no LIST, and the only write is the final record, which is the existing PUT, now up to 4 MiB larger.

Memory: the take reserves, besides the 16 MiB record buffer, 4 MiB for the state, 1 MiB for a document, 384 KiB for a marker and 1.5 MiB for the settings read and its write form, each with an `// ALLOC:` bound. The record must still fit 16 MiB: with the state read first, the shard capture is refused 400 `snapshot_too_large` before its work when the state plus the worst-case record of the shards asked for would not fit. Limit tests: a state at 4 MiB and over it; 4 MiB of state with the maximum shards.

The point in time is per register and per set, and consistent within one set per marker read; not atomic across sets or with the shards.

### The restore

`include_global_state` (a boolean, default false; stock's restore default is also false) is read; every other refused key stays refused. With `true`, and the permission above, after every restored index is whole (and the aliases of ADR 0065), the restore applies what the record holds: each document is put by name into its set through the same validator as the PUT route (a document that no longer validates goes to the failed list), and the persistent settings are merged into the cluster settings register key by key through `ClusterSettings::apply`, the snapshot's value winning. Nothing is deleted. A snapshot without global state answers `"global_state_restored":0`.

This **Differs** from stock (`RestoreService`) on purpose, each listed in `docs/compatibility.md`: settings are merged where stock replaces them; sets are put by name where stock replaces each custom wholesale; the state is applied after the indices where stock applies it with them; the settings are not validated against a settings registry (there is none, and the cluster config holds whatever an operator puts in it, so a restore into a different deployment brings along any deployment-specific key such as an endpoint or a node attribute); a restored template does not apply to indices restored in the same request; failure is partial (below).

A restore whose `indices` selects none, with `include_global_state: true`, applies only the global state and answers 200. Before any write the restore reads each target set's marker and the settings (6 GETs) and refuses 400 `illegal_argument_exception`, with a message naming the set, if a set would pass 1,000 documents or the settings 256 KiB.

Cost per document: the marker read, the content PUT, one marker swap and a delete of a replaced blob, at most 4 requests; a swap that loses its CAS after the attempts goes to the failed list. Budget rows state the exact counts for 0 and 1 documents; the bound is about 4 x 5,000 plus the settings. A foreground restore applies sequentially inside the request's own time budget; what it cannot finish is in the failed list. 

Failure: a failure here never removes an index. A synchronous answer is 200 with `"global_state_restored":N` and `"failed_global_state":["index_template/<name>", ...]` in the `snapshot` object. A background restore (ADR 0063) logs one line to standard error and adds one to its failed count; its global-state step honours cancel (skip the rest, log, keep the indices), a panic in it must not run the guard that deletes the indices (they are complete), and its buffers are part of the thread's reservation; each has a test. A retry of the restore is refused (the indices exist), so the failed list is the only record and the documents are put by hand with their own APIs; this is a **Differs**.

### Both implementations

The Rust node writes and reads the member and builds the setting, the take and the restore. The Java shell changes `SnapshotRecord` (opaque carry, adoption path), the take, the restore and `SnapshotRequestBodies` (`include_global_state` leaves `RESTORE_REFUSED_KEYS`), reading its own stores the same way. Fixtures under `fixtures/snapshots/` gain a record with global state written by each side and read by the other, the Java rewrite round trip, and corrupt cases. The key table, restore-default-flags, `docs/compatibility.md`, the take and restore specs, the budget table, `docs/invariants.md` (the embedded state is a never-lose case), the OpenAPI document and the crate READMEs change with it.

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

A snapshot can carry the operator's configuration and a restore can bring it back to a new deployment, for callers holding the global-state permission. The cost is paid only by a take that asks for it with the setting on, and by a restore that asks for it. The setting stays off until the Java carry-through has shipped; the tree check keeps the fixtures honest, and flipping the setting is an operator decision.
