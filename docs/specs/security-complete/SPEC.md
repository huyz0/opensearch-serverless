# Security plugin, complete and native

## Serves

The "Security (config, authn, authz)" row of [java-dependencies](../../java-dependencies.md): every part of the stock security plugin the serverless scope needs, native, with no Java in the runtime path (Java stays a test oracle and a comparison rig). Builds on [security-config](../security-config/SPEC.md), [security-authn](../security-authn/SPEC.md), [security-authz](../security-authz/SPEC.md) and [ADR 0043](../../adr/0043-security-configuration-as-document-sets.md). Where a part is a separate backend (JWT, OIDC, LDAP), it is a native-tier plugin under [ADR 0080](../../adr/0080-one-plugin-system-three-tiers.md) only if it needs a library the node should not link; otherwise it is a module of the node.

## Slices

Each slice is one PR with its own acceptance criteria below (added when the slice starts), and each security-sensitive one waits for the owner's go before merge.

| Slice | Content | State |
|---|---|---|
| 1 | Management API completeness: list form (`GET` with no name) and `PATCH` (one document, and the bulk form) for the five kinds | this change |
| 2 | Built-in documents: `reserved`, `hidden` and `static` entries shipped as data (the stock `static_roles`, `static_action_groups`), refusal to change a reserved one, `hidden` never listed | next |
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

## Slice 1 change

- `GET /_plugins/_security/api/{kind}` answers `{ "<name>": <document as a read renders it>, ... }` for every stored document of the kind, in name order, `{}` for none. Each document is rendered exactly as the by-name read renders it (a user's `hash` is `""`).
- `PATCH /_plugins/_security/api/{kind}/{name}` applies an RFC 6902 array (`add`, `remove`, `replace`, `test`) to the stored document, validates the result as a `PUT` body is validated, and stores it. A name that is not stored is `404`. The answer is the update acknowledgement of a `PUT` that replaced.
- `PATCH /_plugins/_security/api/{kind}` applies an array of operations whose `path` starts with a document name (`/name` as a whole document, `/name/key` inside it): `add` creates (or replaces) a document from its value, `remove` deletes it, `replace` replaces one that exists. Every operation is validated before any is written; the answer is `{"status":"OK","message":"Resources updated."}`.
- Not in this change: `hidden` and `reserved` (slice 2), `move` and `copy` operations (`400`), atomicity across documents of one bulk patch (each document is one marker swap; see Risks).

⚠️ The plugin's answers for these two forms were not recorded from a live 3.10.0 node (the cloud sandbox has none); they follow the plugin source. A fixture recorded later may move a message; the status codes and shapes are the contract here.

## Acceptance criteria

- AC-1: A list of an empty kind is `200 {}`; a list of stored documents has each document's by-name rendering under its name, in name order, and no hash. Tests: `a_list_holds_every_document_as_a_read_renders_it`, `an_empty_kind_lists_as_an_empty_object`.
- AC-2: A list makes 1 marker GET plus 1 GET a document, no LIST, no PUT, and holds one document at a time. Test: `a_list_costs_a_marker_and_a_get_a_document`.
- AC-3: A list whose answer would pass 4 MiB is refused with a typed error and nothing is truncated. Test: `a_list_over_the_bound_is_refused_whole`.
- AC-4: `PATCH` of one document applies `add`, `replace`, `remove` and `test`, stores the validated result, and refuses an unknown operation, a missing path, a failed `test` and a result that no `PUT` would accept (`400`, document unchanged). Tests: `patch_applies_each_operation`, `a_patch_that_makes_an_invalid_document_changes_nothing`, `a_patch_of_the_wrong_shape_is_400`.
- AC-5: `PATCH` of an absent name is `404` and writes nothing; a patch to a user that sets `password` stores only a hash. Tests: `a_patch_of_an_absent_document_is_404`, `a_patched_password_is_stored_as_a_hash`.
- AC-6: The bulk form creates, replaces, removes and edits inside documents, validates every operation before the first write (an invalid one writes nothing), refuses more than 100 documents, and refuses a mapping for an absent role. Tests: `a_bulk_patch_changes_several_documents`, `an_invalid_operation_in_a_bulk_patch_writes_nothing`, `a_bulk_patch_over_100_documents_is_refused`.
- AC-7: A single patch is what a replacing `PUT` is: 3 GETs (marker and document, then the marker again to swap), 2 PUTs (bytes and marker), no LIST. Test: `a_patch_costs_what_a_replacing_put_costs`.
- AC-8: Both forms are administrator-only like the rest of the API, and every registered method of the new routes has a row in the action table. Tests: `a_user_without_admin_access_is_refused_list_and_patch`, `every_registered_route_has_an_action_row`.
- AC-9: With the switch off the new routes are not registered. Test: the existing `security_is_off_by_default_and_set_by_its_variable` plus `with_security_off_no_guard_is_installed`.

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

None new: the routes exist whenever `serverless.security.enabled` does, which is off by default. No downside to measure on the request path, so no switch of its own.

## Risks

A bulk patch is not atomic across documents: a crash after the first write leaves the earlier documents changed. The plugin applies one configuration index write. Each document is still a single compare-and-swap, and every operation is validated first, so only a store failure can leave a partial result. The list's 4 MiB bound is ours; the plugin has none.
