# Security authentication

## Serves

R15.2, after [security-config](../security-config/SPEC.md): who a request is, from HTTP basic credentials and the stored users, roles and mappings. Decision record: [ADR 0043](../../adr/0043-security-configuration-as-document-sets.md) (fail closed, deny by default, bounded bcrypt, revocation). Authorization (what the user may do) is the next spec.

## Change

- `oss-rest`: a `Guard` on the router runs before the route is looked up, for every request including one no route serves, and may refuse it with its own status, body and `WWW-Authenticate`. An admitted request carries the name it acts as (`Request::principal`).
- `oss-coordinator` (`security/`): reading a `Basic` header, the configuration model built from the five sets (users, roles, mappings, action groups, tenants), which roles a user holds (a mapping's `users`, `backend_roles`, `and_backend_roles`, the user's `opendistro_security_roles`), and the `authinfo` answer.
- `oss-node`: a node snapshot of the five sets, a bounded bcrypt check, and the guard over them, installed when `serverless.security.enabled`. One static administrator comes from the node's settings (`OSS_SECURITY_ADMIN_USER`, `OSS_SECURITY_ADMIN_HASH`: a bcrypt hash, never a password); it holds the role `all_access` and exists before any document is stored, so the management API can be reached on an empty store. With security on and no administrator configured the node refuses to start. `GET /_plugins/_security/authinfo` answers for the caller.

Not in this change: authorization of any request (an authenticated user is not yet limited), tenants' enforcement, `whoami` for certificates, and any scheme but basic. ⚠️ Until authorization lands, security on admits every valid user to every route.

## Acceptance criteria

- AC-1: The guard sees every request before routing: a path no route serves, an unknown method and a request with a body are all refused when the guard refuses, a refused request with a body closes the connection, and an admitted one reaches its handler with its name. Tests: `crates/rest/tests/guard.rs`.
- AC-2: A request with no credentials, a wrong password, an unknown user or a malformed header is `401` with the text `Unauthorized` and a `WWW-Authenticate: Basic` challenge, identical for each. The recorded steps replay. Tests: `an_assembled_node_refuses_every_request_without_valid_credentials_alike`, the 401 steps of `security-access.json` in `crates/compat/tests/node_cluster.rs`.
- AC-3: A user with a stored hash authenticates with the password that hashes to it; a user created through the API authenticates at once on the node that created it. Test: `a_user_created_through_the_api_authenticates_at_once_on_that_node`.
- AC-4: `authinfo` for a mapped user answers the recorded body (`user`, `user_name`, `backend_roles`, `roles`, `tenants`). Tests: the `whoami` step of `security-access.json`, `a_user_holds_the_roles_its_mappings_name`.
- AC-5: With no snapshot (the store cannot be read) every request is `503`, never allowed; a snapshot older than three refresh intervals is treated as none. Tests: `a_node_that_cannot_read_the_configuration_answers_503`, `a_snapshot_older_than_three_intervals_is_not_used`.
- AC-6: At most 4 password checks run at once, each on the connection worker that took its slot; a request that finds it full is `429`; 5 wrong passwords for a user in 10 s make the next attempts `429` until the window ends, a correct password is not throttled by another user's failures, and a failed check is never cached. Tests: `a_full_verification_pool_answers_429`, `five_wrong_passwords_throttle_that_user_only`, `a_failed_check_is_never_cached`.
- AC-7: A cached check is re-validated on every hit: a deleted user or a changed password stops authenticating when the node's snapshot moves (at once for a change through this node, within one interval for another), and the cache holds at most 4096 entries. Tests: `a_deleted_user_stops_authenticating_on_that_node_at_once`, `a_change_on_another_node_is_seen_after_the_interval`, `the_credential_cache_is_bounded`.
- AC-8: A request inside the interval makes no store request; one after it makes at most 5 marker GETs; an idle node makes none; nothing lists. Test: `the_snapshot_refreshes_lazily_and_idle_nodes_make_no_requests`.
- AC-9: With security on and no administrator the node refuses to start, and with security off no request needs credentials and the guard is not installed. Tests: `security_on_without_an_administrator_does_not_start`, `with_security_off_no_guard_is_installed`.
- AC-10: The snapshot's age is read from a monotonic clock, and a clock that has gone back counts the copy as old, so a node never keeps a copy fresh by a clock step. While the stores are unreadable a request does not read them again before the one-second back-off ends, whether or not a change was made here. One request reads while the others are served the held copy, unless a change made through this node is newer than it. Tests: `a_clock_that_goes_back_does_not_keep_a_copy_fresh`, `a_change_made_here_does_not_read_the_stores_on_every_request_while_they_are_down`, `a_change_made_here_is_seen_by_the_next_request_even_inside_the_interval`.
- AC-11: A name that is not a user costs a check against a decoy hash and is counted and throttled like a known name, so neither timing nor `429` tells them apart; failures are tracked for at most 4096 names and a full table never drops a name still inside its window. A stored hash asking for a cost above 15 is a wrong password and is not run. A static administrator hash that is not bcrypt stops the node starting. Test: `an_unknown_name_is_throttled_like_a_known_one`; the hash check is in `security_on_without_an_administrator_does_not_start`.

## S3 request budget

| Operation | PUT-class | GET-class | LIST | Depth |
|---|---|---|---|---|
| a request inside the interval | 0 | 0 | 0 | 0 |
| the first request after the interval | 0 | 5 markers, plus 1 a changed document | 0 | 1 |
| idle | 0 | 0 | 0 | 0 |

## Switch

`serverless.security.enabled` (default off), `serverless.security.refresh_interval` (default 10 s, minimum 1 s), `OSS_SECURITY_ADMIN_USER` and `OSS_SECURITY_ADMIN_HASH`. On by default waits for the ADR's measurement.

## Risks

A credential check costs about 250 ms of one of 4 threads at cost 12, so 4 concurrent first logins by different users saturate the pool for that time and a fifth gets `429`; a client that retries recovers. The 10 s window means a revoked user on another node keeps access for up to one interval.

Known limits, from the independent review: a framing error (conflicting lengths, an oversized declared body, `Expect`) is answered before the guard, so it tells an unauthenticated client the size limit; a kind whose retained documents pass the 1 MiB cache is read through every interval, which costs a GET a document, not the five markers of the budget above; the bcrypt check holds a connection worker for its duration; 5 wrong passwords lock the static administrator out of a fresh login for 10 s (a cached hit still works); the pass-cache is cleared whole when full and not when the users set moves.
