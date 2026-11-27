# Store usage of the write log

## Serves

Roadmap R13.3: the per-index ledger ([store-usage](../store-usage/SPEC.md), [wiring](../store-usage-wiring/SPEC.md)) shares a log blob's PUT among the indices in it, as the shell's node log does with `NodeLogBlob.Header.bytesByIndex`. Without it every write's PUT reads as `_node` cost and an index's line holds only its own descriptors and commits.

## Change

- `NodeLogStore::with_usage(ledger)`: each blob appended, hedged or not, is a request shared among the indices its slices name, by the bytes of those slices. The weights are read from the blob's own header, so the writer's append path takes no new argument. A hedged PUT runs on another thread and shares inside its job.
- With it off (no ledger) the log does exactly what it did and parses nothing.
- A boot with `OSS_STORE_USAGE` on gives the node's log the ledger.
- Not in this change: node packs (not written yet) and a bucket manifest swap.

## Acceptance criteria

- AC-1: a blob of three records of index `a` and one of `b` costs one PUT: `b` takes its rounded-down share by slice bytes, `a` the rest, `_node` nothing, and the bytes written add up to the meter's.
- AC-2: the same holds when the PUT is hedged and runs on another thread.
- AC-3: a blob whose slices name no index is `_node`'s.
- AC-4: a log without a ledger charges nothing to an index.

## Test plan

`crates/writer/tests/it/node_log_usage.rs`. AC-2 was seen failing with the job running the PUT unshared.

## S3 request budget

Unchanged. The header is parsed from the bytes already in memory; one allocation an index in the blob, only with a ledger.

## Switch

None of its own: it rides `OSS_STORE_USAGE` (default off).

## Risks

- A slice without the index's name (a log written by a node that did not know it) is `_node`'s.
- Reading the header again costs CPU on every append with the ledger on; the weights could be handed down from the window encoder instead.
