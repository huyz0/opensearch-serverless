# Change stripes

## Serves

The plan's metadata-writes milestone, fourth part. Writers will confirm that the indexes they write are unchanged by reading one of 16 registers, not one descriptor per index. Follows [descriptor-store](../descriptor-store/SPEC.md).

Invariants served:
- **I-1:** every store call is metered.
- **Fencing:** a stripe changes only by register compare-and-swap, never by a lock or a leader.
- **Requests scale with nodes, not indexes:** the confirming read is per stripe. The writer side that uses it comes with the write path.

## Change

Decision record: [ADR 0008](../../adr/0008-metadata-crate.md) (Accepted).

Ported from huyz0/OpenSearch `serverless/shell/src/main/java/org/opensearch/serverless/metadata/` at `3920dc36`: `ChangeStripes.java`, and the stripe calls in `DescriptorStore.java` (`recordChangesIn`, `changed`, `aboutToDelete`).

### In `oss-format`

`ChangeStripe` is the stripe's value, read and written byte for byte as the Java's `State`.

- **Text layout:** a head line `v1 <epoch> <generation>`, then one line per entry, `<generation> D|P <name>`.
- **Operations:**
  - `parse` is as lenient as the Java's `split`s.
  - `write` writes into the caller's buffer.
  - `add` is the Java's bump logic: keep the newest 256 applied entries and every pending delete. Past 64 pending deletes, move to a new epoch and drop every entry.
- **Constants and keys:**
  - `stripe_of` is the Java's `floorMod(name.hashCode(), 16)` over UTF-16.
  - `change_stripe_key` gives `<base>index-changes/stripe-<nn>`.
  - `MAX_CHANGE_STRIPE_BYTES` bounds a stripe whose names are at most 255 bytes: 89,325 bytes.

### In `oss-meta`

`ChangeStripes` holds `bump(name, kind)` and `read(stripe) -> Snapshot`.

- **`bump`:**
  - It is a register read and then a compare-and-swap. A stripe that does not exist yet is created instead of swapped.
  - It retries up to 64 times, backing off 1 to 32 ms as the Java does. It then answers `MetaError::Contended`.
  - It refuses a name longer than 255 bytes or holding a newline before sending anything. Such a name would make the stripe unbounded or corrupt; the Java has neither guard.
- **`Snapshot::unchanged_since`** is the Java's confirmation rule.
- **`DescriptorStore::record_changes_in(stripes)`** turns recording on, as the Java's `recordChangesIn`:
  - An applied `update` bumps `Changed`. A failure to record is dropped, because the update already applied.
  - `delete` and `delete_if_unchanged` bump `PendingDelete` first. A failure there refuses the delete.
  - Once the delete is applied, refused or failed, they bump `Deleted`, and a failure there is dropped.
  - `create` records nothing, as in the Java.
  - A name no stripe can hold is not recorded: one over 255 bytes, or one holding a newline. `update` and `delete` skip the name rules, as in the Java, so such a name can still reach them. No index created through the rules has such a name, so no writer confirms it by its stripe. Its delete stays idempotent instead of failing the pending bump.
- **Off by default.** Without `record_changes_in`, every operation sends what it sent before.

### Not in this change

The writer's fence that reads stripes, `onChanged` and the routing cache.

## Acceptance criteria

- AC-1: Bumps write the Java's bytes. The format tests replay the bumps of each Java-written recording in `fixtures/change-stripes/` and get the recording's exact bytes. Each recording reads back and writes again unchanged. `stripe_of` matches the Java on every name in `stripe-of.txt`. Malformed stripes are refused. The longest stripe is exactly `MAX_CHANGE_STRIPE_BYTES`.
- AC-2: The confirmation rule holds on each backend, as in the Java's `ChangeStripesTests`:
  - a change to another index in the stripe leaves an index confirmed;
  - an unfinished delete is never confirmed, even past 300 other changes, until its `Deleted` bump;
  - an update does not finish a delete;
  - a read older than every kept change is not confirmed;
  - 65 pending deletes start a new epoch.
- AC-3: Requests, counted on each backend:
  - a `read` is 1 `Get`;
  - a `bump` of an absent stripe is 1 `Get` and 1 `Put`;
  - a `bump` of an existing stripe is 1 `Get` and 1 `Put`, plus 1 `GetRange` on fs, which reads the generation under its lock;
  - each attempt lost to another bump repeats the `Get` and the swap;
  - a bump that loses every attempt answers `Contended`;
  - an invalid name, or a stripe of 16 or more, sends nothing.
- AC-4: With `record_changes_in` on each backend:
  - an applied update bumps `Changed` and a refused update bumps nothing;
  - `delete_if_unchanged` leaves a finished delete, not a pending one, in both delete modes, and so does `delete`;
  - a failed `PendingDelete` bump refuses the delete and leaves the register untouched;
  - a failed `Deleted` bump still answers the delete's own result;
  - a failed delete still finishes its pending entry;
  - a name no stripe holds is deleted without being recorded;
  - without `record_changes_in`, the counts in `counts.rs` are unchanged.

## Test plan

- AC-1: `crates/format/tests/change_stripes.rs` against `fixtures/change-stripes/`, which is written by `Stripes.java` running the Java's `State` code.
- AC-2 to AC-4: `crates/meta/tests/descriptors/changes.rs` over the three backends, with exact meter counts. A store wrapper conflicts or fails chosen requests.
- Planted defects, each seen failing:
  - trimming pending entries along with applied ones;
  - a `Deleted` bump that does not finish the pending entry;
  - a delete whose failed `PendingDelete` bump is ignored.

## S3 request budget

| Operation | PUT-class | GET-class | LIST | DELETE | Depth |
|---|---|---|---|---|---|
| `read` | 0 | 1 | 0 | 0 | 1 |
| `bump`, uncontended | 1 | 1 (and 1 ranged on fs) | 0 | 0 | 2 |
| `update` with recording | +1 | +1 | 0 | 0 | +2 |
| `delete_if_unchanged` with recording | +2 | +2 | 0 | 0 | +4 |

The bumps are paid at lifecycle rate: mapping updates and deletes. What they buy is that writers confirm indexes with one `Get` per stripe instead of one per index. A bump never adds a LIST.

## Switch

None needed: recording is off unless a caller passes `record_changes_in`, and nothing in the tree does yet. The writer change that relies on it will turn it on for every node together, as the Java's `MetadataPlane` does.

## Risks

- A stripe is a hot register under a burst of updates to names in it. 64 attempts with backoff bound one bump to about 2 seconds before it answers `Contended`. A `PendingDelete` that cannot land refuses its delete, which is safe.
- `bump` and `read` allocate their stripe buffers: two of 89 KB for a bump, one for a read. They do not reserve from the node budget. That is fine at the rate of mapping updates and deletes. Before the writer's fence calls `read` per stripe on its write path, `read` must take a pooled buffer.
- A process that crashes between a delete's two bumps leaves a pending entry. It is kept until 64 pile up and the epoch moves on, and until then writers read that index's descriptor directly. This is safe, as in the Java.
