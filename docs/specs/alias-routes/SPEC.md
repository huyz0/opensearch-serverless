# Alias routes: add, remove and read the aliases of named indices

## Serves

R14.4 (aliases over REST). The alias records and their crash-safe mutation protocol exist ([alias-records](../alias-records/SPEC.md), [alias-mutations](../alias-mutations/SPEC.md)); no route reaches them. Invariants served: every store call is metered, requests scale with the aliases a request names and never with the indices or aliases that exist, no LIST, fencing by compare-and-swap.

## Change

`oss_node::alias_route::AliasService` and its routes, all over literal names:

- `PUT|POST /{index}/_alias/{name}`: adds `index` to alias `name`, creating the alias. Body optional: `is_write_index` (bool).
- `DELETE /{index}/_alias/{name}`: removes `index` from `name`; the alias is deleted with its last member.
- `POST /_aliases`: `{"actions":[{"add":{..}},{"remove":{..}}]}` with `index` or `indices`, `alias` or `aliases` (literal names), and `is_write_index` on `add`. Actions run in order; each is atomic, the list is not (**Differs**).
- `GET|HEAD /_alias/{name}`: the members of alias `name`, `{"<index>":{"aliases":{"<name>":{}}}}` (`"is_write_index":true` on the write member). A missing alias is 404 `{"error":"alias [x] missing","status":404}`.
- `GET /{index}/_alias` and `GET /{index}/_alias/{name}`: the aliases of one index, read from its descriptor's hint and confirmed in each alias's register.
- Response to a change: `{"acknowledged":true}`.

**Refused** (501 `unsupported_alias_option`): `filter`, `routing`, `index_routing`, `search_routing`, `is_hidden`, `must_exist`, `remove_index`; wildcards, `_all` and comma lists in a name (they need a listing); `GET /_alias` with no name (it needs a listing). A missing index is 404 `index_not_found_exception`; an alias name that is an index's name, or a data stream's, is 400 `invalid_alias_name_exception`; removing a member that is not there is 404 `aliases_not_found_exception`. Without a role that holds `all_access` the route is refused by the authorization table (no row).

## Acceptance criteria

- AC-1: `PUT /{index}/_alias/{name}` creates the alias; `GET /_alias/{name}` lists the index; `HEAD /_alias/{name}` is 200; the index's descriptor hint names the alias.
- AC-2: adding a second index extends the alias; `is_write_index` marks the member and the read shows it; setting it on another member moves it.
- AC-3: `DELETE` removes one member and keeps the alias; removing the last member deletes the alias (`HEAD` is 404) and clears the hint.
- AC-4: `POST /_aliases` applies add and remove actions in order, `indices` and `aliases` arrays expanding to each pair.
- AC-5: `GET /{index}/_alias` answers the aliases of that index, and `GET /{index}/_alias/{name}` only the named one; an alias the index is not in is 404.
- AC-6: the refusals above answer with the stated status and change nothing.
- AC-7: a request makes no LIST, and an add of one index to an alias costs a bounded number of requests that does not grow with the alias's existing members (asserted as a count on the emulator).

## Tests

`crates/node/tests/server/alias_route.rs`.

## S3 request budget

A change: the alias mutation's own requests (reserve, one prepare read and swap for the added member, publish). A read: one register read for an alias, or one for the index plus one a hinted alias. Zero LIST.

## Switch

None: a new route that nothing else calls and that changes no existing answer.

## Risks

- The list of `POST /_aliases` is not atomic across actions; an error in action k leaves actions before it applied and says which failed.
- Searches, gets and writes through an alias already resolve it; this change adds the way to make one.
