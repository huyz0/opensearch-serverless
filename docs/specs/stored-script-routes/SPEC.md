# Stored scripts: store, read and remove scripts by id

## Serves

R14.4 (stored scripts over REST). The Java shell keeps stored scripts as a document set under `scripts/` (`serverless/shell/.../rest/StoredScriptHandler.java`, `template/StoredScriptStore.java`); a Rust node and a Java node share the set, so each reads what the other stored. Invariants served: every store call is metered; no LIST (the marker names every document, [doc-sets](../doc-sets/SPEC.md)); fencing by compare-and-swap of the marker; bounded memory.

## Change

- `oss_format::DocKind::StoredScript`, container `scripts/`.
- `oss_node::script_route::ScriptService` and its routes: `PUT|POST /_scripts/{id}`, `GET /_scripts/{id}`, `DELETE /_scripts/{id}`.
- A body is `{"script":{"lang":..,"source":..,"options":{..}}}`. `lang` is `painless`, `mustache` or `expression`; `source` is a non-empty string of at most 65,535 bytes (core's `script.max_size_in_bytes`); `options` may hold only `content_type` (a string). The set stores `{"script":{"lang","source"[,"options"]}}`, the form the Java writes.
- `GET` answers `{"_id":"<id>","found":true,"script":{"lang","source"[,"options"]}}`; a missing id is 404 `{"_id":"<id>","found":false}`. `PUT` and `DELETE` answer `{"acknowledged":true}`; deleting a missing id is 404 `resource_not_found_exception`.
- **Refused:** `/_scripts/{id}/{context}` 501 (naming a context makes core compile the script, which this node cannot do); an id beginning with `_` 501; an unknown `lang` 400 `illegal_argument_exception` `script_lang not supported [x]`; a source over the size limit 400; a body that is not that shape 400. **Differs:** a script is stored without being compiled, so one that does not compile is refused where it is first used, not where it is stored. No script is run by this change.
- The routes need `all_access`; no new setting.

## Acceptance criteria

- AC-1: a script is put, read back, replaced and deleted; the stored bytes are the Java's form.
- AC-2: `GET` of a missing id is 404 with `found:false`; `DELETE` of a missing id is 404.
- AC-3: a script written under the Java's marker and content layout reads through the route (the set is `scripts/`).
- AC-4: the refusals above answer with the stated status and store nothing.
- AC-5: a put costs 1 GET and 2 PUTs, a read 2 GETs, and none lists.

## Tests

`crates/node/tests/server/script_route.rs`.

## S3 request budget

As [doc-sets](../doc-sets/SPEC.md): put 1 GET and 2 PUTs, replace adds 1 DELETE, read 2 GETs, delete 1 GET, 1 PUT and 1 DELETE. No LIST.

## Switch

None: a new route that nothing else calls and changes no existing answer.

## Risks

- A script stored here that fails to compile is accepted; a later `_search/template` or script use reports it. The Java compiles at put.
