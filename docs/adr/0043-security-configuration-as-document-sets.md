# 0043: Security configuration is five document sets, enforced at the edge from a node cache

**Status:** Accepted after independent review, 2026-11-24

## Context

The roadmap puts a subset of the security plugin native: roles, role mappings, internal users, tenants and action groups, basic authentication, and index and cluster permissions. Stock keeps this in the `.opendistro_security` system index and a cluster-state-fed cache; there is neither here. The Java shell has no store for it (its probes ran the real plugin over its own index shim). Recorded from the stock 3.10.0 node with the real plugin: `fixtures/compat/security/` (management API answers, 401 and 403 bodies, the denied wildcard, `authinfo`).

Forces, from the principles: S3 requests scale with nodes and bytes (never with requests served), idle nodes make none, memory is bounded, and the target is about 10M tenants. A request-time marker read per call would cost a GET per API call; a per-tenant role would put 10M documents in one set.

## Decision

- Each kind is a [document set](0031-document-sets.md): `security/roles/`, `security/rolesmapping/`, `security/internalusers/`, `security/tenants/` and `security/actiongroups/` (new `DocKind`s). The document is the stock management API's request body, validated as the plugin validates it; the answer is rendered from it. The cap of 1000 a kind stands: this is operator configuration (cluster-wide roles and a few service users), not a per-tenant record. A population larger than that authenticates by backend roles through a later token scheme, and that is out of this subset.
- Passwords are stored only as a bcrypt hash (`hash`), as the plugin does; a `password` in a request is hashed on the way in and never stored or returned (`"hash": ""` in every answer).
- Pure logic lives in `oss-coordinator` (`security/`, beside `template_admin`: the model, body validation, answer rendering, pattern and action-group matching, the decision), with no store or clock; it needs the JSON reader of `oss-doc`, which `oss-rest` (a layer below) cannot depend on. The stores, the route handlers and the cache live in `oss-node` beside the template routes, wired at assembly.
- A node holds a snapshot of the sets it needs in a `DocSetCache`, refreshed lazily: a request that finds the snapshot older than `serverless.security.refresh_interval` (default 10 s) re-reads the markers of the kinds in use, only when traffic arrives. A change made through this node refreshes its snapshot at once. Requests are therefore at most 5 marker GETs a node an interval while serving, and none idle; the delay for a change to reach another node is at most the interval (stock is near immediate). That is a recorded difference.
- A successful password check is cached by user and a keyed hash of the credentials, for no longer than the refresh interval and at most 4096 entries, so bcrypt (about 250 ms at cost 12) runs once per user per interval, not per request.
- Authentication is HTTP basic. A request with none, a wrong password or an unknown user is `401` with the text body `Unauthorized`, as recorded. Authorization takes the action name the stock plugin would assign (`indices:data/read/search`, `indices:data/write/bulk[s]`, `indices:admin/create`, `cluster:monitor/health`...) and the concrete indices the coordinator resolved; a request is refused whole if any resolved index lacks the permission (the recorded wildcard case). The refusal is the recorded 403 `security_exception`.
- The management API itself requires the role set for admin access; the plugin's `restapi.roles_enabled` is the setting `serverless.security.admin_roles` (default `all_access`).
- **A static administrator** comes from the node's settings (a name and a bcrypt hash, never a password), holds `all_access` and exists before any document is stored; with security on and none configured the node refuses to start. This is how the first role or user can be created.
- **Fail closed.** A node with no snapshot, or whose refresh failed and whose snapshot is older than three intervals, answers every request `503` and never allows. Inside three intervals a failed refresh keeps the last good snapshot and retries on the next request.
- **Deny by default.** With security on, authentication runs before routing and body parsing for every route, health, root and `_cat` included, and node-to-node requests are authenticated by the transport's own authenticator, not by this. The unauthenticated allowlist is empty. A test enumerates the registered routes and fails for any that skips the check. A route that resolves no index is checked against cluster permissions.
- **Bcrypt is bounded and off the worker.** Verification runs on a small blocking pool (at most 4 at once; a request that finds it full is `429`), a failed attempt is never cached, failures for a user are throttled (5 in 10 s, then `429` for the rest of the window), and the 401 is identical for an unknown user and a wrong password. Comparison is constant time.
- **Revocation.** A cached check is keyed by user and a keyed hash of the credentials, and every hit is re-validated against the current snapshot (the user exists, its hash is the one verified, its roles are read afresh), so a deleted user or changed password stops working when the node's snapshot changes: at once for a change made on this node, within an interval for another. The cache is dropped whenever the users set's version moves.
- **Bounded memory.** The snapshot and the credential cache are reserved from a named pool before they grow (`// ALLOC:` on each, 1000 documents of at most 1 MiB a kind and 4096 cached checks); a failed reservation refuses the request, never skips the check. `refresh_interval` has a minimum of 1 s, so it cannot become a read a request.
- Basic credentials are sent in the clear without TLS: the node refuses to start with security on over plain HTTP unless `serverless.security.allow_plain_http` is set, for a test or a TLS-terminating proxy.
- Nothing changes with the feature off: `serverless.security.enabled`, default off. It stays off until a measurement shows the per-request cost (one snapshot check, one hash lookup) is within the write p99 budget and the 401/403 paths cannot be reached unauthenticated beyond the one hash check.

## Consequences

- A revocation reaches every node within one interval; an operator needing sooner lowers it at the cost of more marker reads.
- The authorization check needs resolved index names, so it sits in the coordinator's index resolution as a trait defined in `oss-types`, implemented by the node. Slice 3 specifies it.
- Tenants (the Dashboards multi-tenancy) are stored and returned but not enforced.
- Rules out per-tenant security documents, a store read on the request path, and any scheme where revocation is immediate without a request per call.
- Adds a bcrypt dependency (a crate dependency decision, made here; deny.toml and the licence are checked by `cargo deny`).
