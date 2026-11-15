# Descriptor store: index descriptors on registers

## Serves

The plan's metadata-writes milestone, first part: an index's descriptor is created, read, updated and deleted as the register `indices/<name>`, the way the Java shell's `DescriptorStore` does it, so Rust and Java nodes on one bucket agree on which indexes exist. Invariant I-1 (every store call metered), the fencing rule (name uniqueness and every change arbitrated by register compare-and-swap, never a lock or a leader), and the S3 rule (no LIST: a name's key is derived).

## Change

Decision record: [ADR 0008](../../adr/0008-metadata-crate.md) (Proposed), for the new crate and its layer.

Ported from huyz0/OpenSearch `serverless/shell/src/main/java/org/opensearch/serverless/` at `3920dc36`: `metadata/DescriptorStore.java`, `metadata/Tombstones.java`, `metadata/Names.java`, `metadata/RegisterMap.java` and `cluster/IndexDescriptor.java` (`toBytes`, `fromStream`).

- `oss-format` writes what it already reads:
  - `IndexRecord`, the whole descriptor with every field `toBytes` writes, parsed and written in the Java's field order and with Jackson 3's escaping, so a descriptor the Java wrote comes back out byte for byte. The settings and the unit layout are kept as their JSON object bytes and written back as they are; building them from typed settings is the settings registry's work.
  - The tombstone record (`{"tombstone":true,"deleted_at":N}`, plus `"reaping_at":M` once a sweep claims it), and the hourly marker bucket name (`uuuuMMddHH`, UTC).
  - Both write into the caller's buffer and say how long a buffer they needed when it is short; nothing allocates.
- A new crate, `oss-meta` (layer 2, between the stores and the roles):
  - `Names`: the Java's index-or-alias name rules, with its messages.
  - `DescriptorStore`: `create`, `get`, `resolve`, `generation_of`, `update`, `delete` and `delete_if_unchanged`, over any `BlobStore`. Create is one conditional create for a new name. Only a taken name is read: a tombstone is swapped over, unless a sweep claimed it under a minute ago; anything else is a name already taken. Delete-if-unchanged is a conditional delete where the store honours one, otherwise a marker under `tombstones/<hour>/<name>` and then a swap to a tombstone, so a recreated name keeps counting generations up. The caller says which (the Java probes; the probe comes later).
- Not in this change: listing, the tombstone sweep, change stripes, the conditional-delete probe, alias records beyond telling one apart, and wiring into a role.

## Acceptance criteria

- AC-1: the Java's descriptor register `fixtures/shell-blobs/indices/fixture` parses as an `IndexRecord` and writes back to the same bytes; a record with every field set writes the bytes the generator `fixtures/jackson/` recorded from Jackson 3, and parses back to an equal record.
- AC-2: a string holding every character below U+0080 and some above writes as Jackson 3's `JsonGenerator.writeString` writes it, configured as OpenSearch's `JsonXContent` (recorded in `fixtures/jackson/`): `\b \t \n \f \r` short, other controls `\u00XX` in upper case, `"` and `\` escaped, `/` and everything from U+007F up raw.
- AC-3: parsing refuses what the Java refuses (no name, no uuid, a shard count below 1 or above 4,096, a bucketed index with more than one shard or a unit layout, a repeated known field, malformed JSON) and also, where the Java would coerce, a value of the wrong JSON type; unknown fields are skipped.
- AC-4: tombstones write as the Java's and parse their two times; the bucket of an instant is its UTC hour, checked at the epoch, a leap day, the end of 9999 and an instant just before an hour.
- AC-5: a name breaking each of the Java's rules is refused by create, get, resolve and generation_of before any request, with the Java's message; a valid one passes.
- AC-6: on the emulator, S3 over loopback and fs: create then get returns the record; a second create of the name is `AlreadyExists`; create over a tombstone applies at a later generation; create over a tombstone a sweep claimed under a minute ago is `BeingReclaimed`, and after the minute applies; update with the current generation applies and with a stale one returns none; delete-if-unchanged in both modes removes the index (get is none), refuses a stale generation, and in tombstone mode leaves the marker key; delete is idempotent; resolve tells an index, an alias and an absent name apart, with the generation; a create of a name holding a record longer than the caller's buffer is `AlreadyExists`, as the Java's; update and delete take a name the rules refuse today, as the Java's do, so a name created under older rules can still be removed.
- AC-7: requests, counted on the meter on each backend: create of a new name 1 `Put`; create of a taken name 1 `Put` and 1 `Get`; create over a tombstone 2 `Put` and 1 `Get` on the emulator and S3, plus 1 `GetRange` on fs; get, resolve and generation_of 1 `Get`; delete-if-unchanged in conditional mode 1 `Delete` (plus 1 `GetRange` when the generation was not just read or written), in tombstone mode 2 `Put` (plus that `GetRange`); no `List` anywhere.
- AC-8: a buffer too short for the record is refused with how long one is needed, and nothing is sent.

## Test plan

- AC-1 to AC-4: `crates/format/tests/records.rs`, against the fixture and the Jackson recordings; seen failing against a planted escape of `/` and a planted field-order swap.
- AC-5: unit tests in `crates/meta/src/names.rs`, one per rule, with messages compared to the Java's text.
- AC-6, AC-7, AC-8: `crates/meta/tests/descriptors/`, one suite over the three backends, exact meter counts per backend; seen failing against a planted create that reads first and a planted delete that skips the marker.

## S3 request budget

| Operation | PUT-class | GET-class | LIST | Depth |
|---|---|---|---|---|
| `create`, new name | 1 | 0 | 0 | 1 |
| `create`, taken name | 1 | 1 | 0 | 2 |
| `create`, over a tombstone | 2 | 1 | 0 | 3 |
| `get`, `resolve`, `generation_of` | 0 | 1 | 0 | 1 |
| `update` | 1 | 1 ranged GET unless the generation is remembered | 0 | 1 or 2 |
| `delete` | 0 (DELETE) | 0 | 0 | 1 |
| `delete_if_unchanged`, conditional | 0 (DELETE) | 1 ranged GET unless remembered | 0 | 1 or 2 |
| `delete_if_unchanged`, tombstone | 2 | 1 ranged GET unless remembered | 0 | 2 or 3 |

The Java's counts, except that its swaps read the whole register where these read 8 bytes, and its delete is a batch DeleteObjects where this is one DELETE.

## Switch

None: nothing calls the store yet. The delete mode is the caller's, as the Java's `setConditionalDelete`.

## Risks

- The settings and unit-layout objects are written back as the bytes they were read from. A record built in Rust carries whatever settings bytes its caller gives (checked to be one JSON object); if they are not what `Settings.toXContent` would write, a Java node reads them the same, but a byte comparison between the two writers fails. The settings registry replaces them with a typed writer.
- A repeated unknown field is skipped where Jackson refuses the whole record (`STRICT_DUPLICATE_DETECTION`): telling needs every name kept, and no writer repeats one.
- Every read takes the caller's buffer, as `BlobStore::read_register` does: a descriptor larger than it is refused, not truncated.
- `DeleteMode::Conditional` is trusted as configured until the Java's `ConditionalDeleteProbe` is ported: on a store that ignores `If-Match` on DELETE it deletes unconditionally. Nothing configures the store yet; the first caller that does uses `Tombstone` unless the store was probed.
- A create that finds the name taken reads it into a tombstone-sized buffer: on S3 the GET is still one request, but a live record is refused as too long rather than read whole.
