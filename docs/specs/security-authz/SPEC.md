# Security authorization

## Serves

R15.2, after [security-authn](../security-authn/SPEC.md): what an authenticated user may do. Decision record: [ADR 0043](../../adr/0043-security-configuration-as-document-sets.md) (deny by default); the two design choices below, the action table and the post-routing check, get their own ADR before code.

## Change

- `oss-rest`: an `Authorizer` on the router is asked, right after a route is found and before the content-type, parameter and body checks, with the principal, the method, the route's pattern and its decoded path parameters, and may refuse with its own status and body (a refused request with a body closes the connection). `Router::routes()` lists every registered method and pattern, a prefix route as any method.
- `oss-coordinator` (`security/authz`): the decision, from the roles' `cluster_permissions` and `index_permissions`, the action groups (built-in and stored, to any depth, a cycle ends, a stored one never replaces a built-in), the built-in roles of the stock plugin as shipped data, and the plugin's `*` and `?` wildcard rule. A request element with `*` or `?` is covered only by a pattern that matches every string it stands for.
- `oss-node`: the action table, a row for each registered route and method (`Open`, a cluster action, an index action over the path's `{index}`, or `AllAccess`), the authorizer over the node's snapshot, and the recorded 403.

Not in this change: tenants' enforcement, document-level and field-level security, `/regex/` patterns, resolving aliases and wildcards to concrete indices, per-item checks in a bulk body, a `terms` lookup in a search body naming another index.

## Acceptance criteria

- AC-1: A user granted `read` on `logs-*` may search `logs-1`; `other` and any action the role does not grant are refused. Tests: `a_granted_pattern_allows_its_names_and_only_them`, the `search allowed` and `search denied` steps of `security-access.json`.
- AC-2: A wildcard request is covered only by a pattern that matches everything it stands for (`a?` does not cover `a*`; `l*` and `*` are not covered by `logs-*`; no index means `*`), a comma list needs every element covered, and a pattern's `*` never spans the comma. Tests: `a_wildcard_request_needs_a_pattern_that_covers_it`, `every_index_of_a_list_must_be_covered`, `covering_agrees_with_every_string_the_request_stands_for` (brute force), and at the route `a_comma_list_or_encoded_name_is_checked_element_by_element`.
- AC-3: Cluster permissions and groups decide cluster actions; a `_bulk` needs the cluster permission `indices:data/write/bulk` and `all_access` on indices. Tests: `cluster_permissions_and_action_groups_decide_cluster_actions`, `bulk_needs_the_cluster_permission_and_all_access_on_indices`.
- AC-4: A user without `indices:admin/delete` or `indices:admin/create` is refused with the recorded 403 body naming the action, and `cluster:admin/settings/update` likewise. Tests: the `delete index denied`, `create index denied`, `wildcard search` and `cluster settings denied` steps replayed on a node.
- AC-5: Action groups expand to any depth, a cycle ends, a name that is no group is an action, and a stored group or role never replaces a built-in. Tests: `groups_expand_to_any_depth_and_a_cycle_ends`, `a_stored_group_or_role_does_not_replace_a_built_in`.
- AC-6: Every registered route and method has a row in the action table, and a route with none is `AllAccess` at run time. Tests: `every_registered_route_has_an_action_row`, `a_route_without_a_row_needs_all_access`.
- AC-7: The administrator is allowed everything; a user name holding a wildcard voids a `${user.name}` pattern. Test: `the_administrator_may_do_everything_and_user_names_in_patterns_are_literal`.
- AC-8: A refused request is refused before its parameters, content type and body are looked at (a bad query parameter is still 403), and a refused request with a body closes the connection. Tests: `crates/rest/tests/authorizer.rs`.
- AC-9: With security off no authorizer is installed. Test: `with_security_off_no_guard_is_installed`.
- AC-10: A prefix (plugin) route and every body-carried-index route is refused to a user without `all_access`, for every method. Test: `routes_whose_indices_are_in_the_body_or_that_are_proxied_need_all_access`.

## S3 request budget

None: no operation reads the store beyond the authn snapshot.

## Switch

`serverless.security.enabled`. Authorization is not separately switched: a node that authenticates without authorizing is the open door this change closes.

## Risks

The action table is the stock mapping of REST routes to transport actions, kept by hand, and a wrong name grants or denies the wrong thing; each row is exercised by a test that sends the route as a limited user. The refusals of aliases, wildcards that resolve to allowed indices, and body-carried indices are stricter than the plugin, and an alias made onto an index reaches it for whoever holds the alias's name; both are in [compatibility](../../compatibility.md).
