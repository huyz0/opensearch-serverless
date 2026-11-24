# A forwarded group is held once, not three times

## Serves

R13.1: a big bulk is parsed in slices charged to the bulk budget ([bulk](../bulk/SPEC.md)). A slice's shard groups bound for another node ([forward](../forward/SPEC.md), [node-bulk-handler](../node-bulk-handler/SPEC.md)) were then copied before they were sent, copies the budget never counted, so a node forwarding many large slices held about three times what it had been granted.

## What copied

- Sender: `cluster_owner::operation` copied each item's source into an owned `String`; `signed_call` then encoded the whole group into a `Vec` that grew by doubling, so while it grew the old and new buffers were both live. Peak over the parsed slice: the group's sources again, plus up to twice the message.
- Owner: `bulk_handler` decoded the frame into `BulkOperation`s, then cloned every source again into the engine's `Op`.

## Change

- `BulkOperation` and `BulkRequest` borrow their strings (`Cow`): a decoded request owns them, a request built from a slice's items borrows each source from the parsed item. The bytes on the wire are unchanged.
- `BulkRequest::encoded_size_hint` and `signed_call` take the message's capacity up front, so the encode allocates once.
- The owner moves each decoded source into the engine's `Op` instead of cloning it.

No setting: nothing is slower and no byte on the wire changes (principles: no downside, no switch).

## Acceptance criteria

- AC-1: the bytes of a forwarded request built from borrowed items equal those of the same request with owned strings.
- AC-2: forwarding a group of 16 MiB of sources (four items) through the cluster routes peaks at most 1.25 times the group's size above the parsed items; before the change it peaked at about 3 times (observed failing first).
- AC-3: the owner applies a forwarded group exactly as before (the existing `bulk_handler` tests pass unchanged).
- AC-4: the size hint is at least the encoded length for any request, so the encode never reallocates.

## Test plan

`crates/node/tests/forward_peak.rs` (own binary: the counting allocator is process-wide) for AC-2; `crates/transport/tests/wire/forwarding.rs` for AC-1 and AC-4; the existing role tests for AC-3.

## S3 request budget

None: no store request is added or removed.

## Risks

A borrowed request cannot outlive the items it borrows; the forward call is synchronous, so it does not. The owner-side saving is not measured by a peak test (the engine's own allocation drowns it); it is one fewer clone by construction.
