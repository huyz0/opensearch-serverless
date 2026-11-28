# 0090: A token-authenticated request carries an identity, with backend roles from the token

**Status:** Accepted after independent review, 2026-11-28 (its findings are folded into the text below)

## Context

[ADR 0043](0043-security-configuration-as-document-sets.md) authenticates HTTP basic against stored internal users, and the guard hands the router one thing: the user's name. Every later decision (authorization by [ADR 0051](0051-authorization-at-the-route-by-action-table.md), `authinfo`, `account`, the management API's admin check) looks the name up in the snapshot to find its backend roles and, through the role mappings, its roles.

A JWT user is not in the snapshot. Its name is the token's subject, and its backend roles are a claim in the token (`roles_key`). Stock maps those backend roles through the same role mappings, so a token holding `"roles":["analyst"]` gets every role whose mapping names the backend role `analyst`. The name alone cannot carry that: two tokens for one subject may hold different roles at once, and a name-keyed side table would let one token's roles apply to another's request.

The key and the claims' names are configuration. The effective `securityconfig` is code ([slice 3](../specs/security-complete/SPEC.md)), so there is no stored domain to read them from; they must come from node settings, as the static administrator does.

## Decision

**Identity.**
- The guard's verdict carries an `Arc<Identity>`, not a bare name: `Verdict::Allow(Option<Arc<Identity>>)`. `Identity` is the name plus a source: `Stored` (a user of the snapshot; its backend roles are read from the snapshot as now) or `Token` (its backend roles are the ones the token's claim gave, fixed for this request). The source stops a token from falling back to a name lookup, and an empty roles claim from looking like a stored user. The identity is shared by `Arc`, so a request clones a reference count.
- `Request::principal()` and the authorizer's `principal()` keep returning the name, so the consumers that only need a name (the SQL, plugin, pin, index and async-search routes) do not change. A new `identity()` returns the whole.
- **`Config::roles_of_identity(&Identity)` is the one role entry point**; the name-only `roles_of` becomes private to it. Every caller (the admin check, `cluster_allows`, `all_access`, `indices_allow`, `authinfo`, `account`, the 403 text) goes through it, and a test lists the callers. A token's roles are the roles whose mapping names one of its backend roles, or all of the `and_backend_roles`; the mapping's `users` list never applies to a token (not by name and not as `*`). The token's roles are never copied into a stored user's record.
- A token whose subject names a stored user (checked against the snapshot on every request), the static administrator, or is empty or not a string is refused (`401`, the one body). The comparison is exact. A user created later makes existing tokens for that name fail within the refresh interval.

**Configuration** is node settings, not documents. `OSS_SECURITY_JWT_*`: `KEY` (an HMAC secret, base64, of at least the hash's size), or `PUBLIC_KEY` (a PEM, a separate setting, so a public key is never read as a secret), `SUBJECT_KEY` (`sub`), `ROLES_KEY` (none; a top-level claim, a string or an array of at most 64 strings of at most 256 bytes, anything else refuses the token), `ISSUER`, `AUDIENCE` (a string or an array in the token), `SKEW_SECONDS` (30) and `MAX_LIFETIME_SECONDS` (none; when set a token whose `exp` is further than that from now is refused). The key and the PEM are secrets and never appear in a settings read or in `securityconfig`. No key set means no JWT domain and nothing changes. The token is read from the `Authorization: Bearer` header only; a URL parameter would put it in logs and proxies and is not offered.

**Verification** is `ring`, declared as a direct dependency of `oss-node` (it is already in the build through rustls; the layering and `cargo deny` checks apply), in `oss-node` (`security_jwt`); the claim rules and the identity's role resolution are pure and live in `oss-coordinator/security`.
- The configured key decides the algorithm: an HMAC secret allows HS256/384/512 by its length, an RSA public key (at least 2048 bits) RS256/384/512, a P-256 key ES256 and a P-384 key ES384 (raw `r||s`). The header's `alg` is compared with that before any crypto; `none`, a mismatch and anything else is refused. `kid`, `jku`, `x5u`, `jwk` and `typ` are ignored; an unknown `crit` is refused.
- `exp` and the subject are required; `nbf`, the issuer and the audience are checked when present or configured, with the skew. A token over 8 KiB is refused unread; the header and payload are parsed by the bounded JSON reader of `oss-doc` with its depth and key limits, and the decoded buffers are bounded by the 8 KiB and carry `// ALLOC:`. Token text never reaches a log or an error body.
- Over plain HTTP the node already refuses to start with security on unless `serverless.security.allow_plain_http` is set; that covers a bearer token as it covers basic.

**Refusal and throttling.** A failed token is the recorded `401 Unauthorized` (text), one body for every cause, with `WWW-Authenticate: Bearer` added to the basic challenge when a domain is configured. Failures are throttled by the existing bounded failure map (4096 names), keyed by the token's *claimed* subject only after its signature verifies is it a name; a token that fails verification is counted under one shared `jwt` bucket, so a flood of forged subjects grows nothing and cannot lock a real user out of basic. When that bucket is over its limit (5 in 10 s, as for passwords) the request is `429` before any signature is checked, so verification cost is bounded. A valid token neither counts toward the bucket nor resets it.

**Order.** A `Basic` header is basic, a `Bearer` header is a token; there is no other source. A token never reaches bcrypt.

## Consequences

- A token cannot be revoked before `exp`; a short lifetime, or `MAX_LIFETIME_SECONDS`, is the control. Rotating the key is a restart of every node, and removing the key invalidates every token. A role mapping change reaches a token request within the refresh interval, as for basic.
- The `oss-rest` guard, request and authorizer types change once, for every later backend (OIDC, LDAP, client certificates all produce an identity). That is a decision here, not only an expectation.
- Requests cost no store request and no outbound call. A signature check is microseconds for HMAC and ECDSA and tens of microseconds for RSA, run on the worker; no cache is needed, so no memory is held per token. The figures are measured on the emulator rig and are provisional.
- Settings are per node and must agree across nodes: a node with another key refuses tokens the others accept. Stock keeps them in one cluster-wide document; here they are deployment configuration, like the administrator.
- Switch: the key being set is the switch. Both modes are tested, and the existing basic tests keep pinning basic.
- Tests use an independent oracle: tokens minted by a separate tool, recorded as fixtures, never by the verifier. Planted tokens cover `alg: none`, a public key used as an HMAC secret, expired, `nbf` in the future, wrong issuer and audience, tampered and truncated signatures, oversize, a subject equal to a stored user and to the administrator, two tokens for one subject with different roles served together, and a mapping change within the interval; and a count shows a token request makes no store request.
- Docs updated with the slice: `docs/compatibility.md` (no `jwks_uri`, no JWE, per-node settings, header only), the settings how-to and the `oss-rest` README.
- Not in this slice: `jwks_uri` (slice 6's shared fetcher), JWE, the on-behalf-of token API, and a URL parameter.
