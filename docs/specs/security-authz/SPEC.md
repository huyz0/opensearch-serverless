# Security authorization

## Serves

R15.2, after [security-authn](../security-authn/SPEC.md): what an authenticated user may do. Decision record: [ADR 0043](../../adr/0043-security-configuration-as-document-sets.md) (deny by default); the two design choices below, the action table and the post-routing check, get their own ADR before code.

## Change

- `oss-rest`: a route may carry an action name and say where its indices come from (a path parameter, or none). After the route is found and before its handler runs, an `Authorizer` on the router is asked, with the principal, the action and the indices, and may refuse with its own status and body. `Router::routes()` lists what is registered so a test can enumerate it.
- `oss-coordinator` (`security/`): the decision. A user's permissions are the union of its roles' `cluster_permissions` and `index_permissions` (action groups expanded, to any depth, cycles ignored), matched by the stock plugin's wildcard rule; an index request is allowed only if every index it names matches the `index_patterns` of one permission that allows the action. A pattern or request index with `*` is matched literally as a wildcard, so a wildcard request is allowed only by a pattern that covers it (the recorded `wildcard search` denial). `${user.name}` in a pattern is substituted.
- `oss-node`: the action table, one row for each registered route and method: the stock action name (`indices:data/read/search`, `indices:admin/create`, `cluster:admin/settings/update`, ...) and where its indices are. A registered route with no row, or a request whose indices cannot be known from the path (a `_bulk` or `_msearch` body, `_mget` items), is allowed only to a user holding the action on `*`: deny by default. A test fails the build if a registered route has no row. The 403 body is the recorded `security_exception`.

Not in this change: tenants' enforcement (`securitytenant`), document-level and field-level security, index patterns that are regular expressions (`/.../`), and per-item checks inside a bulk body.

## Acceptance criteria

- AC-1: A user whose roles grant `read` on `logs-*` may `_search` `logs-1` and is refused `other` with the recorded 403 body naming `indices:data/read/search` and the user. Tests: the `search allowed` and `search denied` steps of `security-access.json`.
- AC-2: A wildcard request is allowed only by a pattern that covers it: `compat-sec-*` against `compat-sec-logs-*` is refused. Test: the `wildcard search` step.
- AC-3: A role holding `write` and the `indices:data/write/bulk` cluster permission may index; without the cluster permission a `_bulk` is refused. Tests: the `index allowed` step, a unit test of the cluster-permission rule.
- AC-4: A user without `indices:admin/delete` or `indices:admin/create` is refused with the action named; the `cluster settings denied` step is `cluster:admin/settings/update`.
- AC-5: Action groups expand to any depth, a cycle terminates, and an unknown group name is read as an action name.
- AC-6: Every registered route and method has a row in the action table. Test: enumerates `Router::routes()`.
- AC-7: A route whose indices are not in its path is allowed only with the action on `*`; an administrator (`all_access`) is allowed everything.
- AC-8: With security off the authorizer is not installed and no request is refused.
- AC-9: The decision makes no store request: it reads the node snapshot only.

## S3 request budget

None: no operation reads the store beyond the authn snapshot.

## Switch

`serverless.security.enabled`, as authentication. Authorization is not separately switched: a node that authenticates without authorizing is the open door this change closes.

## Risks

The action table is the ported stock mapping of REST routes to transport actions, kept by hand: a row that names the wrong action grants or denies the wrong thing, so each row is covered by a test that sends the route as a limited user. A body-carried index is checked as `*`, which is stricter than the plugin; that difference is documented in [compatibility](../../compatibility.md).
