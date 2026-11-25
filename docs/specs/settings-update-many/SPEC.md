# A settings update over several indices

## Serves

Parity plan R14.4: the forms of the settings update route that name more than one index ([settings-update-route](../settings-update-route/SPEC.md) takes one).

## Change

`PUT /_settings` (every index, capped), and `PUT /{index}/_settings` where `{index}` is a comma list of names, trailing-`*` prefix patterns, `_all`, `*` or aliases. The body is read and checked once, before any index changes; each index it resolves to is then updated as one index is.

## Acceptance criteria

- AC-1: `PUT /{a},{b}/_settings` updates both and answers `{"acknowledged":true}`; each descriptor's settings version moves by one.
- AC-2: `PUT /logs-*/_settings` updates the indices whose names start with `logs-`; one that is no index (a tombstone) is skipped. A pattern that matches nothing is acknowledged and changes nothing.
- AC-3: an alias stands for its members; an index named twice, or by a name and a pattern, is updated once.
- AC-4: `PUT /_settings`, `/_all/_settings` and `/*/_settings` update every index, up to the cap of 1,000 (`SettingsService::with_pattern_cap` lowers it); more than the cap is refused 400 `too_many_indices` and nothing changes.
- AC-5: a body the update refuses, or a name that is no index (not from a pattern), is refused before any index changes: 400 for the body, 404 `index_not_found_exception` for the name.
- AC-6: a pattern that is not a trailing `*` is refused 501 `unsupported_setting` naming it, before any index changes.

## Test plan

`crates/node/tests/server/settings_route.rs`: one test per criterion on the emulator rig, reading each descriptor back.

## S3 request budget

A name costs what one index costs (1 GET and 1 PUT, an unchanged one 1 GET). A pattern, `_all` and `*` cost one capped listing of at most 1,000 names (`// LIST:`) and one GET of each match to drop what is no index, then the same per index; no more than the cap's worth of indices is read or written. The listing is priced as PUTs, so a pattern is an admin operation, never on a request path of data.

## Bounds

At most 1,000 indices a request; the names are held once, one record buffer pair reused index by index.

## Switch

None: the one-index form's behaviour with more names.

## Risks

Not atomic across indices: a store failure part-way leaves the indices before it updated (core applies a cluster-state change at once). The body and every explicit name are checked before the first write, so only a store failure can do it, and a repeat of the same request finishes the rest (an update that changes nothing is acknowledged and writes nothing).
