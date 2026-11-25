# Data streams: create, read and delete one by name

## Serves

R14.4 (data streams over REST). The record (`oss_format::AliasRecord` with `data_stream`, `generation`, `timestamp_field`), the template's `data_stream` object and the bulk route's create-only rule exist; no route makes, shows or removes a stream. The Java shell's `DataStreamHandler` (`serverless/shell/.../indexadmin/DataStreamHandler.java`) is the port. A stream is an alias record with a generation over backing indices named `.ds-<name>-<six-digit generation>`, so a Rust node and a Java node share the streams. Invariants served: every store call is metered; no LIST (every name is literal); fencing by compare-and-swap (the alias protocol, the descriptor create); bounded memory.

## Change

- `oss_node::data_stream_route::DataStreamService` and the routes `PUT|GET|DELETE /_data_stream/{name}`.
- **PUT.** The name is checked as an index name (the shell's rules, [oss_meta::names]) and must be lowercase; it must not begin with `_` (501) or `.` (400). The template that wins the name must declare `data_stream`, else 400 `illegal_argument_exception` "no matching index template found for data stream [name]". The first backing index `.ds-<name>-000001` is created through the same path as `PUT /{index}`, inheriting the template that won the **stream's** name (not the backing name), with the timestamp field (`timestamp_field.name`, else `@timestamp`) mapped as a `date` unless the template maps it. Then the alias record `{indices:[backing], data_stream:true, generation:1, timestamp_field}` is created through the alias protocol. Answers `{"acknowledged":true}`. A name that is taken by an index or alias is 400 `resource_already_exists_exception`; when the record loses that race the backing index is deleted again.
- **GET.** Answers `{"data_streams":[{"name","timestamp_field":{"name"},"indices":[{"index_name","index_uuid"}],"generation","status","template"}]}` as core renders it: `status` is `GREEN`, or `RED` when a backing index has no descriptor (its `index_uuid` is `_na_`); `template` is the name of the template that wins the stream's name, left out when none does. A name that is not a stream is 404 `resource_not_found_exception` "data stream [name] does not exist".
- **DELETE.** The stream's record is deleted by compare-and-swap first, so a write arriving meanwhile has nowhere to land, then each backing index the record named, by the uuid read before, through the index delete. Answers `{"acknowledged":true}`; a missing name is 404; a record that changed since it was read is 409 `version_conflict_engine_exception` and nothing is deleted.
- `DELETE /{index}` of a name beginning `.ds-` that is the write index of a stream is refused 400 `illegal_argument_exception` "index [x] is the write index for data stream [y] and cannot be deleted" (`oss_coordinator::indices::admin::delete_check`). An index not beginning `.ds-` makes no extra request.
- **Refused 501:** a wildcard, `_all` or a comma list in the name (each would need a listing; name one stream), and `GET /_data_stream` with no name. Query parameters `timeout`, `master_timeout`, `cluster_manager_timeout` are accepted and ignored; `expand_wildcards` is not.
- **Differs:** an invalid name's message is the shell's index-name text, and the backing index is not given core's `_data_stream_timestamp` mapper, so a document without the timestamp field is not refused for lacking it. A write through the name must still be a create (the bulk route's rule). Rollover is not here.
- The routes need `all_access`; no new setting.

## Acceptance criteria

- AC-1: a stream is created, read back and deleted; the record stored is the Java's form and its backing index `.ds-<name>-000001` exists with the timestamp field mapped as a date and the template's settings and mappings applied.
- AC-2: PUT without a matching data-stream template, a name that is taken, a name beginning `_` or `.`, or an invalid or uppercase name is refused with the stated status and creates nothing.
- AC-3: GET of a missing name is 404; the body names `index_uuid` as the backing descriptor's uuid, `status` `RED` with `_na_` for a backing index that has no descriptor, and `template`.
- AC-4: DELETE removes the record and every backing index; a missing name, or a name that is an index or a plain alias, is 404.
- AC-5: a stream the Java wrote (record and backing indices in the Java's form) reads and deletes through the route.
- AC-6: `DELETE /{index}` of a stream's write index is refused 400 and deletes nothing; of an older backing index it deletes.
- AC-7: wildcards, `_all`, comma lists and `GET /_data_stream` are refused 501 and make no store request.
- AC-8: the PUT, GET and DELETE of a one-index stream and the write-index guard cost what the `data_stream.*` and `index.delete.write_guard` rows of [request-budgets](../../request-budgets.md) say; a PUT and a GET never list.

## Tests

`crates/node/tests/server/data_stream_route.rs`.

## S3 request budget

Measured on the in-process emulator (provisional), with templates stored and one backing index: PUT 13 requests (the name read, the template markers and documents, the backing index's create, the alias create with its hint swap); GET 4 GETs (the record, the backing descriptor, the template markers and documents); DELETE the record read, the backing descriptor read, the record's conditional delete and hint removal, then the index delete of each backing index (which lists, as `DELETE /{index}` does). A GET and a DELETE scale with the backing indices the stream has, which only rollover grows; no request scales with the streams, indices or aliases that exist. A stale record at DELETE (409) is not covered by a test.

## Switch

None: a new route that nothing else calls and changes no existing answer, except that the delete of a stream's write index is now refused where it silently orphaned the stream.

## Risks

- A descriptor read per backing index grows with rollovers. A stream with thousands of backing indices makes a slow GET and DELETE. Core answers the uuids too; a bound can be added with rollover.
- The write-index guard trusts the `aliased_by` hints, so in the window between a record's publish and its hint swap a write index is deletable.
- A crash between the backing index's create and the alias create leaves a `.ds-<name>-000001` index no stream names; the next PUT of that name finds the backing name taken and refuses 400 until it is deleted by name. The Java has the same window.
