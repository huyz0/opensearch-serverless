# 0049: Authorization is decided at the route, from an action table, by the names in the path

**Status:** Proposed

## Context

[ADR 0043](0043-security-configuration-as-document-sets.md) puts authentication in front of every request and left authorization to a later slice, sketched as a trait in the coordinator's index resolution, after aliases and wildcards are resolved to concrete indices. The stock plugin decides on resolved names: a request through an alias, or a wildcard, is allowed when every index it resolves to is allowed.

Resolution here is per index and sits behind the coordinator's reads (descriptor lookups that cost store requests). Putting the decision there would make an allowed or refused answer depend on store reads, put the check in every handler, and make the table of which handler checks what impossible to enumerate. The forces are the same as for authentication: no store request on the request path for the check, deny by default, and a rule that can be a predicate over files is a check, not a sentence.

## Decision

- The router knows each route's action. A route is registered with the stock action name (`indices:data/read/search`, `cluster:admin/settings/update`, ...) and where its indices come from (a path parameter, or nowhere). After the route is found and before its handler runs, an `Authorizer` on the router decides from the principal, the action and the index names in the path, and may refuse with its own answer. The decision is a read of the node's snapshot and nothing else: no store request.
- The decision is pure and lives in `oss-coordinator` (`security/authz`): a user's permissions are the union over its roles of `cluster_permissions` and `index_permissions`, with action groups expanded to any depth (a cycle ends; a name that is not a group is an action), built-in action groups and roles shipped as data from the stock plugin's static configuration, and the plugin's wildcard rule (`*` any run, `?` one character, `${user.name}` substituted).
- **Names are matched as written, not resolved.** An index named in the path is allowed only if a permission with the action has a pattern that matches it; a wildcard in the request is allowed only by a pattern that covers it (every string the request matches is matched by the pattern, decided by matching the request as a literal against the pattern and by the pattern being `*`). This is stricter than the plugin where a wildcard or an alias would resolve to allowed indices only. A request through an alias needs a permission on the alias name. The difference is documented in `docs/compatibility.md`.
- **A route whose indices are not in its path** (a `_bulk`, `_msearch` or `_mget` body naming indices, `_search` with no index, a point-in-time id) is allowed only to a user holding the action on `*`: the check is as for the request naming every index. A route with no action row is refused unless the user holds `all_access`'s `*` on both levels. Prefix routes (plugins proxied to Java nodes) have no action and are refused the same way.
- **The table is complete by test.** `Router::routes()` lists every registered method and pattern, and a test fails if one has no row in the node's table. Adding a route without deciding who may call it does not compile into a passing build.
- The refusal is the recorded 403 `security_exception`: `no permissions for [<action>] and User [name=..., backend_roles=[...], requestedTenant=null]`.
- No separate switch: authorization is on whenever `serverless.security.enabled` is. A node that authenticates and does not authorize is the open door this closes.

## Consequences

- A request that the plugin allows through an alias or wildcard may be refused here until the user is granted the alias or the wildcard; never the reverse. Documented, with a way to loosen it later by resolving names, which would need its own ADR and its own request budget.
- Bulk and multi-search bodies are checked as `*`, so a limited user cannot use them. Per-item checks need the body parsed before the decision and are not done.
- Tenants, document-level and field-level security, and `/regex/` patterns are not enforced; a role using them is read for its actions and indices only.
- Adds a data file (the stock static action groups and roles) and a router API (`routes()`, an action on a route, an `Authorizer`).
