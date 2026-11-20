# A writer keeps the indexes it reads, confirmed by the change stripes

## Serves

R7/R8 descriptor cache ([descriptor routes](../node-descriptor-routes/SPEC.md) named it the next piece): `Routes::index` read the descriptor register on every call, so a bulk cost a request per distinct index. [Change stripes](../change-stripes/SPEC.md) exist to confirm many indexes by one read.

## Change

- `oss_node::index_cache::IndexCache` keeps the `IndexInfo` of each index a lookup read, per change stripe (16), at most 8,192 a stripe. An entry is trusted for a window (`OSS_INDEX_CONFIRM_MS`, default 5,000; 0 turns the cache off). Past it, the next lookup reads the stripe the name hashes to, once a window for every index of that stripe, and `Snapshot::unchanged_since` says whether the descriptor can have changed; if not, the entry is confirmed again, else it is dropped and the descriptor is read.
- The stamp an entry carries is the stripe's state taken before the descriptor was read, so a change racing the read is seen by the next confirmation. A confirmation moves the stamp forward, so an entry never ages out of the stripe's kept changes.
- An absent name, an alias and a tombstone are never kept: a create is seen at once. An update or a delete is seen within one window.
- `DescriptorRoutes::with_cache` shares one cache between every route of the node (boot wires it). Every store that updates or deletes a descriptor in the node now records the change in the stripes (`record_changes_in`): the index service and the index delete service. An update then costs a stripe read and a swap more, a delete two.
- A stripe that cannot be read falls back to reading the descriptor and keeping nothing.

## Acceptance criteria

- AC-1: an index read once is answered from memory for a window: a hundred lookups make no request.
- AC-2: past the window an unchanged index costs at most one read of its stripe, shared by every index of it, and no descriptor read; it is then confirmed for another window.
- AC-3: an update is not seen inside the window and is seen after it.
- AC-4: a delete is seen after the window; a recreate under the name is seen at once.
- AC-5: an absent name is never kept.
- AC-6: a change to an index of one stripe does not drop an index of another.

## Test plan

`crates/node/tests/index_cache.rs` over the emulator, with a clock the test moves; observed failing with the confirmation made unconditional.

## S3 request budget

A lookup costs no request in the window; otherwise at most one stripe read a stripe a window (16 a window at most, whatever the indexes or bulks) plus one descriptor read for an index that changed or was never read. An idle node makes none: nothing runs on a timer. Before this a bulk cost one read per distinct index. An update costs one read and one swap more, a delete two (the stripe's bump).

## Switch

`OSS_INDEX_CONFIRM_MS` (default 5,000, 0 = off). The downside is bounded staleness: an update or delete another node makes reaches this node's writes within the window. The shell's resolver has the same trade. Requests fall from one a distinct index a bulk to at most 16 a window.

## Risks

- Every node of a deployment must record descriptor changes in the stripes or a change is unseen here for good. In this node that is the index service and the index delete service; alias changes are not recorded and aliases are not kept.
- A write to an index deleted on another node can land inside the window; the shard's head and incarnation checks catch the rest.
