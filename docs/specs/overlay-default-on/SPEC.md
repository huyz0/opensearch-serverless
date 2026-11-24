# The tail overlay is on by default

## Serves

Parity plan R9. [node-boot-tail](../node-boot-tail/SPEC.md) built the overlay behind `OSS_TAIL_OVERLAY`, off until takeover was covered and the cost was measured; [node-commit-push](../node-commit-push/SPEC.md) and its takeover suite cover the first, the meter below the second. The shell ships it on.

## Change

`serverless.search.tail_overlay.enabled` (`OSS_TAIL_OVERLAY`) defaults to `true`. `index.serverless.search.tail_overlay.enabled` stays the per-index switch (default true, an index that sets false is served from its commit). The commit push's send to other nodes, which defaults to the opposite of the overlay, is off by default with it ([node-commit-push](../node-commit-push/SPEC.md)).

## Acceptance criteria

- AC-1: `NodeConfig` defaults the overlay to on, and `OSS_TAIL_OVERLAY=false` turns it off.
- AC-2: the build rule's bars, measured with the overlay on against off, three nodes, `refresh=false` writes, `everywhere` visibility: the write's ack p99 is within 5% of off's, requests a write do not rise, visibility does not get worse.
- AC-3: the takeover fault suite is green with the overlay on ([node-commit-push](../node-commit-push/SPEC.md) AC-9).
- AC-4: with the overlay on, every existing suite passes as it did; a suite that pinned the old default sets the variable it pinned.

## S3 request budget

Fewer: no commit is published per refresh to make a write visible. See VERIFIED.md for the per-write counts.

## Switch

Both settings stay: the node setting and the per-index one. Default on because the measurements show it wins on everything measured (write ack, visibility, search latency, requests a write); what is not measured is listed in VERIFIED.md and ADR 0034.
