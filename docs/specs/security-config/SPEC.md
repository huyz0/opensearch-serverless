# Security configuration API

## Serves

The security-subset part of R15 (k-NN, security, extensions, mixed-fleet soak): the stores and the management API that authentication and authorization (their own specs, next) read. Decision record: [ADR 0043](../../adr/0043-security-configuration-as-document-sets.md).

## Change

`/_plugins/_security/api/{roles,rolesmapping,internalusers,tenants,actiongroups}/{name}`: `GET`, `PUT` and `DELETE`, over five document sets, validating and answering as the stock plugin 3.10.0 does (`fixtures/compat/security/security-{roles,rolesmapping,internalusers,tenants,actiongroups}.json`, recorded from the real plugin).

- In `oss-format`: five `DocKind`s (`security/roles/` and the rest).
- In `oss-coordinator` (`security/`): the model of each kind, body validation (unknown key, wrong datatype, mandatory and one-of keys, as recorded), the answers (`CREATED`, `OK`, `NOT_FOUND`, `Invalid configuration`) and the rendering of a stored document.
- In `oss-node`: the store over `DocSet`, the routes beside the template routes, and the bcrypt hashing of a `password`. Registered only when `serverless.security.enabled` is on.

Not in this change: authentication, authorization, the list form (`GET` with no name), `PATCH`, `hidden` and `reserved` entries beyond answering `false`, the `securityconfig` API, and tenants' enforcement.

## Acceptance criteria

- AC-1: Replaying the five recorded management fixtures against an assembled node with security on gives the recorded statuses and bodies. Test: `crates/compat/tests/node_cluster.rs`, `security config replay`.
- AC-2: A `password` is stored only as a bcrypt hash that verifies it, and no answer or stored document holds the password. Tests: `a_password_is_stored_only_as_its_hash`, `an_answer_never_carries_a_hash_or_a_password`.
- AC-3: A role mapping for a role that does not exist is a 404 and writes nothing. Test: `a_mapping_for_an_absent_role_is_refused`.
- AC-4: A put is 1 GET and 2 PUTs the first time, and a read within the refresh interval makes no request. Tests: `security_requests_per_operation`, `a_read_inside_the_interval_makes_no_request`.
- AC-5: A management read is direct, so a change through any node is visible to the next read of any node (the snapshot and its interval belong to authentication, next). Test: `a_password_is_stored_only_as_its_hash` (update then read).
- AC-6: The 1001st document of a kind is refused and the set is unchanged. Test: `the_1001st_document_of_a_kind_is_refused`.
- AC-7: With the switch off the routes are not registered and every request makes the requests it made before. Tests: the `security management` replay with the switch off in `crates/compat/tests/node_cluster.rs` (observed failing with `no handler found`), and `security_is_off_by_default_and_set_by_its_variable`.

## Test plan

AC-1 fails first on a node with no route (404 against the recorded 201). AC-2 fails if the hash is the password. AC-3 would catch a mapping written for a role that was deleted. AC-4 and AC-5 would catch a marker read per request. AC-7 would catch a route that outlives its switch.

## S3 request budget

| Operation | PUT-class | GET-class | LIST | DELETE | Depth |
|---|---|---|---|---|---|
| put, first of a name | 2 | 2 | 0 | 0 | 4 |
| put, replace | 2 | 3 | 0 | 1 | 4 |
| put of a mapping | one more read of the role | | | | |
| get | 0 | 2 | 0 | 0 | 2 |
| delete | as [document sets](../doc-sets/SPEC.md) | | 0 | | |

Idle: none.

## Switch

`serverless.security.enabled` (`OSS_SECURITY`): on when an administrator is configured, off otherwise, so no flag is needed (owner rule: plugins need no enable flag or have it on by default). ⚠️ On its own it serves the management API with no authentication until the authentication spec lands, so it must not be turned on in a deployment before then. On by default waits for the measurement named in the ADR.

## Risks

The recorded behaviour is that of one plugin version, 3.10.0; a validation the fixtures do not exercise (field lengths, reserved names) may differ. A password hash costs about 250 ms of one worker; a burst of user creations holds workers (bounded by the worker pool).
