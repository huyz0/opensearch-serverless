# Security plugin, complete and native

## Serves

The "Security (config, authn, authz)" row of [java-dependencies](../../java-dependencies.md): every part of the stock security plugin the serverless scope needs, native, with no Java in the runtime path (Java stays a test oracle and a comparison rig). Builds on [security-config](../security-config/SPEC.md), [security-authn](../security-authn/SPEC.md), [security-authz](../security-authz/SPEC.md) and [ADR 0043](../../adr/0043-security-configuration-as-document-sets.md). Where a part is a separate backend (JWT, OIDC, LDAP), it is a native-tier plugin under [ADR 0080](../../adr/0080-one-plugin-system-three-tiers.md) only if it needs a library the node should not link; otherwise it is a module of the node.

## Slices

Each slice is one PR with its own acceptance criteria below (added when the slice starts), and each security-sensitive one waits for the owner's go before merge.

| Slice | Content | State |
|---|---|---|
| 1 | Management API completeness: list form (`GET` with no name) and `PATCH` (one document, and the bulk form) for the five kinds | this change |
| 2 | Built-in documents: the stock static roles, action groups and `global_tenant` and the reserved roles of the default configuration, shipped as data (`builtin.json`, the recorded lists), listed and read with their flags, changes refused with the recorded 403 | this change |
| 3 | `securityconfig` API (`GET /_plugins/_security/api/securityconfig`, `PUT .../config`), `authc`/`authz` domain configuration as a document | |
| 4 | `authinfo` parity, `whoami`, `account` API (`GET`/`PUT /_plugins/_security/api/account`), `health`, `ssl/certs` | |
| 5 | JWT authentication (HS and RS/ES keys, `jwks`, subject and roles keys, header and URL parameter) | |
| 6 | OIDC (discovery document and JWKS fetched by the node, cached, bounded) | |
| 7 | LDAP and client-certificate authentication (the LDAP client is the native-tier question) | |
| 8 | Document-level and field-level security, field masking | |
| 9 | Audit log (categories, sinks, bounded queue, no request on the request path) | |
| 10 | Dashboards multi-tenancy (tenant selection, `securitytenant` header) | |

SAML is not planned: it needs a browser redirect flow only Dashboards drives. Raise it if wanted.

## Slice 1: list form and PATCH (sections below)

## Slice 2: built-in documents

The recording (`fixtures/compat/security-api/security-api.json`) lists, beside stored documents, 57 roles, 26 action groups and `global_tenant` that the stock node ships: the plugin builds in ten roles, the action groups and `global_tenant` (`static: true`), and its default configuration adds 47 more roles (`reserved: true`). All are shipped here as `builtin.json` in `oss-coordinator`, taken from that list, listed and read with their flags, refused on `PUT`, `PATCH` and `DELETE` (`403`, `Resource 'x' is static.` or `is reserved.`), and the reserved roles also feed authorization (so a mapping to `alerting_full_access` grants what the plugin grants). A bulk patch naming one is refused whole.

Not shipped on purpose: the default configuration's demo users (`admin`, `kibanaserver`, ...), their role mappings and `admin_tenant`: they carry well-known credentials and are installer demo data, not part of the plugin. `hidden` stays `false` (no shipped document is hidden). The list answer is in name order, not the plugin's hash order. A stored document that shares a shipped name is not shown (the shipped one wins, as in authorization).

## Slice 1 change

- `GET /_plugins/_security/api/{kind}` answers `{ "<name>": <document as a read renders it>, ... }` for every stored document of the kind, in name order, `{}` for none. Each document is rendered exactly as the by-name read renders it (a user's `hash` is `""`).
- `PATCH /_plugins/_security/api/{kind}/{name}` applies an RFC 6902 array (`add`, `remove`, `replace`; the plugin supports no more) to the stored document, validates the result as a `PUT` body is validated, and stores it. A name that is not stored is `404`. The answer is the update acknowledgement of a `PUT` that replaced.
- `PATCH /_plugins/_security/api/{kind}` applies an array of operations whose `path` starts with a document name (`/name` as a whole document, `/name/key` inside it): `add` creates (or replaces) a document from its value, `remove` deletes it (a name that is not there changes nothing), `replace` replaces one that exists. Every operation is validated before any is written; the answer is `{"status":"OK","message":"Resource updated."}`, or `"No updates required"` when nothing changed.
- Not in this change: `hidden` and `reserved` (slice 2), `move`, `copy` and `test` operations (`400 Unsupported patch operation`, as recorded), atomicity across documents of one bulk patch (each document is one marker swap; see Risks).

The answers follow the recording of stock 3.10.0 in `fixtures/compat/security-api/security-api.json` (huyz0/opensearch-serverless#996): the update acknowledgements, `Unsupported patch operation: <op>. Supported are: add,remove,replace`, `Missing field "<name>"`, `Array index <n> is out of bounds`, `Wrong request body`, `Resource updated.` and `No updates required`. The recorded lists also hold the plugin's built-in documents, which slice 2 adds.

## Acceptance criteria

- AC-1: A list of an empty kind is `200 {}`; a list of stored documents has each document's by-name rendering under its name, in name order, and no hash. Tests: `a_list_holds_every_document_as_a_read_renders_it`, `an_empty_kind_lists_as_an_empty_object`.
- AC-2: A list makes 1 marker GET plus 1 GET a document, no LIST, no PUT, and holds one document at a time. Test: `a_list_costs_a_marker_and_a_get_a_document`.
- AC-3: A list whose answer would pass 4 MiB is refused with a typed error and nothing is truncated. Test: `a_list_over_the_bound_is_refused_whole`.
- AC-4: `PATCH` of one document applies `add`, `replace` and `remove`, stores the validated result, and refuses an unknown operation, a missing path, a failed `test` and a result that no `PUT` would accept (`400`, document unchanged). Tests: `patch_applies_each_operation`, `a_patch_that_makes_an_invalid_document_changes_nothing`, `a_patch_of_the_wrong_shape_is_400`.
- AC-5: `PATCH` of an absent name is `404` and writes nothing; a patch to a user that sets `password` stores only a hash. Tests: `a_patch_of_an_absent_document_is_404`, `a_patched_password_is_stored_as_a_hash`.
- AC-6: The bulk form creates, replaces, removes and edits inside documents, validates every operation before the first write (an invalid one writes nothing), refuses more than 100 documents, and refuses a mapping for an absent role. Tests: `a_bulk_patch_changes_several_documents`, `an_invalid_operation_in_a_bulk_patch_writes_nothing`, `a_bulk_patch_over_100_documents_is_refused`.
- AC-7: A single patch is what a replacing `PUT` is: 3 GETs (marker and document, then the marker again to swap), 2 PUTs (bytes and marker), no LIST. Test: `a_patch_costs_what_a_replacing_put_costs`.
- AC-8: Both forms are administrator-only like the rest of the API, and every registered method of the new routes has a row in the action table. Tests: `a_user_without_admin_access_is_refused_list_and_patch`, `every_registered_route_has_an_action_row`.
- AC-9: With the switch off the new routes are not registered. Test: the existing `security_is_off_by_default_and_set_by_its_variable` plus `with_security_off_no_guard_is_installed`.
- AC-10: A list and a read by name hold the shipped documents with `reserved` and `static` as recorded, beside stored ones in name order, and a read of a shipped name makes no store request. Tests: `the_built_in_set_is_the_recorded_one`, `a_list_holds_the_built_in_documents_with_their_flags`, `a_built_in_is_read_by_name_without_a_store_request`.
- AC-11: `PUT`, `PATCH` and `DELETE` of a shipped name are `403` with the recorded message and write nothing; a bulk patch naming one writes nothing. Tests: `a_change_to_a_shipped_document_is_refused_as_the_plugin_refuses_it`, `a_built_in_cannot_be_written_patched_or_deleted`, `a_bulk_patch_naming_a_built_in_writes_nothing`.
- AC-12: A mapping for a shipped role is accepted, and a reserved role grants what the plugin grants. Tests: `a_mapping_for_a_built_in_role_is_accepted`, `a_reserved_role_grants_what_the_plugin_grants`.
- AC-13: No user, mapping or demo tenant is shipped. Tests: `no_user_or_mapping_is_shipped`, `no_demo_user_is_shipped`.

## Test plan

Each test is written first and seen to fail (501 for the list, 404 `no handler found` for `PATCH`). AC-3 would catch a list that grows with the set. AC-6 would catch a bulk patch that writes the first document and then fails on the second.

## S3 request budget

| Operation | PUT-class | GET-class | LIST | Depth |
|---|---|---|---|---|
| list of n documents | 0 | 1 + n | 0 | 1 + n, sequential, bounded by the 1,000 cap |
| patch of one document | 2 | 3 | 0 | 4 |
| bulk patch of k documents | 2k | 1 + 2k (one role read per mapping) | 0 | bounded by k |

Idle: none. A list is an administrator's read of operator configuration (at most 1,000 documents a kind), not a data path.

## Switch

None new: the routes exist whenever `serverless.security.enabled` does, which is on whenever an administrator is configured (owner rule: no enable flag for plugins). No downside to measure on the request path, so no switch of its own.

## Risks

A bulk patch is not atomic across documents: a crash after the first write leaves the earlier documents changed. The plugin applies one configuration index write. Each document is still a single compare-and-swap, and every operation is validated first, so only a store failure can leave a partial result. The list's 4 MiB bound is ours; the plugin has none.
