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
- AC-5: A change through a node is visible to its next read; one made through another node is visible once the interval has passed and not before. Test: `a_change_on_another_node_arrives_after_the_interval`.
- AC-6: The 1001st document of a kind is refused and the set is unchanged. Test: `the_1001st_document_of_a_kind_is_refused`.
- AC-8: Next specs (authentication, authorization) carry the acceptance criteria for the ADR's fail-closed, deny-by-default, bounded-bcrypt, revocation and memory rules; this spec's stores must expose the snapshot version they rely on. Test: `the_snapshot_reports_its_version_and_age`.
- AC-7: With the switch off the routes are not registered and every request makes the requests it made before. Test: `with_security_off_the_routes_do_not_exist`.

## Test plan

AC-1 fails first on a node with no route (404 against the recorded 201). AC-2 fails if the hash is the password. AC-3 would catch a mapping written for a role that was deleted. AC-4 and AC-5 would catch a marker read per request. AC-7 would catch a route that outlives its switch.

## S3 request budget

| Operation | PUT-class | GET-class | LIST | DELETE | Depth |
|---|---|---|---|---|---|
| put, first of a kind | 2 | 1 | 0 | 0 | 3 |
| put, replace | 2 | 1 | 0 | 1 | 3 |
| get, delete | as [document sets](../doc-sets/SPEC.md) | | 0 | | |
| a read inside the interval | 0 | 0 | 0 | 0 | 0 |
| a read after it | 0 | 1 a kind in use | 0 | 0 | 1 |

Idle: none.

## Switch

`serverless.security.enabled`, default off; `serverless.security.refresh_interval`, default 10 s. On by default waits for the measurement named in the ADR.

## Risks

The recorded behaviour is that of one plugin version, 3.10.0; a validation the fixtures do not exercise (field lengths, reserved names) may differ. A password hash costs about 250 ms of one worker; a burst of user creations holds workers (bounded by the worker pool).
