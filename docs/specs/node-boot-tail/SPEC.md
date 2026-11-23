# A booted node turns the tail overlay on from its configuration

## Serves

Parity plan R9.3 and R7.6: the tail overlay was built and wired by `Assembled::wire_tail`, but no booted node called it, so the writer's push and the reader's check never ran in a node. Builds on [node-tail-wiring](../node-tail-wiring/SPEC.md), [reader-tail-fresh](../reader-tail-fresh/SPEC.md) and [node-search-fresh](../node-search-fresh/SPEC.md).

## Change

- `OSS_TAIL_OVERLAY` (`true` or `false`, default `false`, the setting `serverless.search.tail_overlay.enabled`) and `OSS_REFRESH_VISIBILITY` (`owner`, `serving` or `everywhere`, default `everywhere`) are read by `NodeConfig::from_lookup`, and refused with the variable's name when unusable.
- `serverless.search.refresh_visibility` is a registered node setting. The mode is `TailParts::visibility`: only `everywhere` makes a reader check with the writer before it answers, and `serving` and `everywhere` make a refresh wait for the search nodes ([node-refresh-wait](../node-refresh-wait/SPEC.md)).
- The tail fetch and push actions are listed on the transport server before it binds, refusing until the roles exist, and given their handlers once the node is assembled. Off, they keep refusing, as an action nobody registered does.
- `Node::tail_on` says whether the overlay is on.

## Acceptance criteria

- AC-1: a node booted with the overlay on, in each visibility, starts, enrols and serves a write and a read, and reports the overlay on.
- AC-2: a node with the setting unset or `false` starts with the overlay off.
- AC-3: the config refuses an unusable value by name, and defaults to off and `everywhere`.
- AC-5: each visibility name picks its mode, and only `everywhere` makes a reader check with the writer.
- AC-4: both tail actions are listed and refuse until their handlers are given; a handler given later answers.

## Test plan

`crates/node/tests/server/boot.rs` (booted nodes over a file store) and the unit tests in `crates/node/src/boot/`.

## S3 request budget

None: no request is added. LIST 0, HEAD 0, PUT 0.

## Risks

- A node with the overlay on pushes acknowledged writes to readers and asks writers for grants; the setting stays off by default until the cost and visibility fixtures are measured.
