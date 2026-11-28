# 0081: Notifications is ported natively; channel configs are document sets with a derived catalogue, and a send costs no store request inside a TTL

**Status:** Accepted (independent review, two rounds, 2026-11-28; the second accepted with three edits, applied)

## Context

Alerting ([ADR 0075](0075-alerting-is-ported-natively.md)) and index state management ([ADR 0077](0077-index-state-management-is-ported-natively.md)) both send notifications, and both today either keep their own channel documents or refuse the `notification` action. The stock Notifications plugin (`opensearch-project/notifications`) is the one place a cluster defines channels (Slack, Chime, Teams, webhook, email, SNS, SES) that the other plugins refer to by id, and its REST routes are `/_plugins/_notifications`. The owner decided on 2026-11-28 that the stock plugins are ported to Rust with no Java in the runtime path (no proxy, no fallback); Java is a test oracle and a perf and memory comparison only. The plugin is not in the proxy's prefix list of [ADR 0046](0046-plugin-requests-are-proxied-to-java-nodes.md) and nothing serves it.

Forces: requests scale with nodes and bytes, never with records ([principles](../principles.md)); the stock plugin's list routes filter and sort every config, which on a document set is one GET a config; a send is work with no request in flight, so it must not add a timer; a send makes the node an HTTP client for its callers, which is a server-side request forgery surface; memory is reserved before it is held ([ADR 0004](0004-bounded-memory-and-streaming.md)).

## Decision

Notifications is native in `oss-format` (two document kinds), `oss-node` (the routes, the catalogue and the sender), as [the spec](../specs/notifications/SPEC.md) lays out.

- **Configs are a document set** (`notifications/configs/`, a marker register naming immutable content blobs, written by compare-and-swap, never listed) with a **derived catalogue**: one document holding the non-secret summary of every config. A list reads the catalogue (3 GETs whatever the number of configs), not each config; a page of `GET /configs` then reads the page's configs (at most 100 more). The catalogue is written by a rebuild-per-attempt update, so concurrent writers do not lose entries. The catalogue is written after the config; a gap between the two swaps is healed by the list (a replace that crashed between them keeps the old summary until the unanswered client retries), which compares the two markers' names and repairs the difference. This bounds a list at a constant while keeping the stock plugin's filters.
- **A send reads its config through a per-node cache** whose freshness is a TTL checked lazily by a send (the shape of the LTR model cache): 0 store requests inside the TTL (30 s), 1 marker GET past it (a second only if the config changed), one flight a config, no timer, no poll. Idle makes no request.
- **The sender is an in-process `Notifier`** that alerting and ISM call, plus the stock `feature/test` route. HTTP channels share one egress guard: resolve once and check every address, refuse private, loopback, link-local and mapped ranges, connect to the address checked, no redirect, bounded body, deadline and concurrency, TLS verified. No retry: the caller owns retry. A delivery failure is a status in the answer, as in the stock plugin.
- **Credentials of email, SNS and SES are the operator's, bound to operator-declared accounts**, never to a tenant-chosen host, role or region, so a tenant cannot aim the node's credentials at a server of its own; a config blob holds no account secret beyond the channel URL the plugin itself stores. The egress guard's switches are node operator settings, not cluster settings.
- It is written as a service with a `routes()` function and no assembly side effect beyond that, so it moves onto [ADR 0080](0080-one-plugin-system-three-tiers.md)'s linked-in native tier as the ISM and LTR ports will, without a rewrite. It does not wait for that system.
- It is off by default (`serverless.notifications.enabled`) until the spec's stock recording and request counts are recorded; with it off the routes are absent.

## Consequences

- Easier: a Rust cluster defines and tests channels without a Java node; alerting and ISM have one place to send through; a list costs a constant.
- Harder: each config write is two swaps and a catalogue rewrite of up to up to about 1.3 MB (bound 2 MiB); plugin-visible shapes are reproduced from documentation until a stock recording exists; the node reaches the network on a caller's behalf, guarded as above.
- Rules out: Java nodes reading the Rust store, the plugin's `.opensearch-notifications-config` index, and channels the plugin does not have.
- Does not change: ADR 0046's proxy list (it never held this prefix).
